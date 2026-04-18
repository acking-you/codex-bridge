//! Structured outbound QQ message definitions.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::reply_context::ActiveReplyContext;

/// Single reply request accepted by the local API and skill-facing CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyRequest {
    /// Active reply token bound to the running task.
    pub token: String,
    /// Plain-text result body.
    pub text: Option<String>,
    /// Image file to send back.
    pub image: Option<PathBuf>,
    /// Generic file to send back.
    pub file: Option<PathBuf>,
    /// Explicit list of QQ ids to `@` in the group reply. When empty or
    /// absent the bridge falls back to `@`-ing the original sender.
    #[serde(default)]
    pub at: Vec<i64>,
    /// Explicit QQ message id to quote with the outbound reply. When absent
    /// the bridge quotes the original inbound message that triggered the
    /// task (preserving current behaviour).
    #[serde(default)]
    pub reply_to: Option<i64>,
}

/// Allowed reply payload forms.
#[allow(missing_docs, reason = "Enum variant fields are self-descriptive transport data.")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyPayload {
    /// Plain-text QQ reply.
    Text(String),
    /// Image attachment reply.
    Image {
        /// Canonical local artifact path.
        path: PathBuf,
    },
    /// File attachment reply.
    File {
        /// Canonical local artifact path.
        path: PathBuf,
        /// File name presented to QQ.
        name: String,
    },
}

/// Final outbound target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboundTarget {
    /// Direct private chat target.
    Private(i64),
    /// Group chat target.
    Group(i64),
}

/// Structured message segment understood by the NapCat transport adapter.
#[allow(missing_docs, reason = "Enum variant fields are self-descriptive transport data.")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboundSegment {
    /// Reply to an existing message.
    Reply {
        /// QQ message id to reference.
        message_id: i64,
    },
    /// Mention one user in group chat.
    At {
        /// QQ identifier of the mentioned user.
        user_id: i64,
    },
    /// Plain-text segment.
    Text {
        /// Plain-text segment body.
        text: String,
    },
    /// Image segment backed by a local artifact file.
    Image {
        /// Canonical local artifact path.
        path: PathBuf,
    },
    /// Generic file segment backed by a local artifact file.
    File {
        /// Canonical local artifact path.
        path: PathBuf,
        /// File name presented to QQ.
        name: String,
    },
}

/// Transport-ready outbound message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundMessage {
    /// Target conversation for the outbound message.
    pub target: OutboundTarget,
    /// Ordered structured segments.
    pub segments: Vec<OutboundSegment>,
}

impl ReplyRequest {
    /// Convert one API/CLI request into a validated reply payload, an
    /// optional explicit `@` target list, and an optional explicit quote
    /// target. `reply_to` is trusted verbatim — when `Some(id)` the bridge
    /// quotes that exact QQ message id instead of the triggering message.
    pub fn into_payload(
        self,
        context: &ActiveReplyContext,
    ) -> Result<(ReplyPayload, Vec<i64>, Option<i64>)> {
        let Self {
            token: _,
            text,
            image,
            file,
            at,
            reply_to,
        } = self;
        let variants = usize::from(text.is_some())
            + usize::from(image.is_some())
            + usize::from(file.is_some());
        if variants != 1 {
            bail!("reply request must contain exactly one of text, image, or file");
        }

        if let Some(text) = text {
            let text = sanitize_inbound_mentions(text.trim());
            if text.is_empty() {
                bail!("reply text must not be empty");
            }
            return Ok((ReplyPayload::Text(text), at, reply_to));
        }

        if let Some(path) = image {
            return Ok((
                ReplyPayload::Image {
                    path: resolve_artifact_path(&path, context)?,
                },
                at,
                reply_to,
            ));
        }

        let path =
            resolve_artifact_path(file.as_ref().expect("checked single payload variant"), context)?;
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            bail!("reply file name is invalid");
        };
        let name = name.to_string();
        Ok((
            ReplyPayload::File {
                path,
                name,
            },
            at,
            reply_to,
        ))
    }
}

/// Build a transport-ready outbound message from the active context.
///
/// `at_targets` — non-empty list of QQ ids to `@`. When empty the bridge
/// falls back to `@`-ing the original sender.
///
/// `reply_to` — explicit quote target. `Some(id)` quotes that exact message
/// id; `None` falls back to `context.source_message_id` (the inbound
/// triggering message).
pub fn build_outbound_message(
    context: &ActiveReplyContext,
    payload: ReplyPayload,
    at_targets: &[i64],
    reply_to: Option<i64>,
) -> OutboundMessage {
    let target = if context.is_group {
        OutboundTarget::Group(context.reply_target_id)
    } else {
        OutboundTarget::Private(context.reply_target_id)
    };

    let mut segments = Vec::new();
    if context.is_group {
        let quoted_id = reply_to.unwrap_or(context.source_message_id);
        segments.push(OutboundSegment::Reply {
            message_id: quoted_id,
        });
        if at_targets.is_empty() {
            segments.push(OutboundSegment::At {
                user_id: context.source_sender_id,
            });
        } else {
            for &user_id in at_targets {
                segments.push(OutboundSegment::At {
                    user_id,
                });
            }
        }
    }

    match payload {
        ReplyPayload::Text(text) => segments.push(OutboundSegment::Text {
            text,
        }),
        ReplyPayload::Image {
            path,
        } => segments.push(OutboundSegment::Image {
            path,
        }),
        ReplyPayload::File {
            path,
            name,
        } => segments.push(OutboundSegment::File {
            path,
            name,
        }),
    }

    OutboundMessage {
        target,
        segments,
    }
}

fn resolve_artifact_path(path: &Path, context: &ActiveReplyContext) -> Result<PathBuf> {
    let raw_path =
        if path.is_absolute() { path.to_path_buf() } else { context.repo_root.join(path) };
    let canonical = raw_path
        .canonicalize()
        .with_context(|| format!("resolve reply path {}", raw_path.display()))?;
    let artifacts_dir = context.artifacts_dir.canonicalize().with_context(|| {
        format!("resolve artifacts directory {}", context.artifacts_dir.display())
    })?;

    if !canonical.starts_with(&artifacts_dir) {
        bail!("reply attachments must stay under {}", artifacts_dir.display());
    }
    if !canonical.is_file() {
        bail!("reply attachment must be an existing file");
    }
    Ok(canonical)
}

/// Rewrite any inbound `@`-mention placeholder that slipped into the agent's
/// reply text before it reaches QQ.
///
/// The bridge injects `@<bot>`, `@<QQ:1234>` and `@nickname<QQ:1234>` markers
/// into received group messages so the agent can read who was addressed.
/// System-prompt guidance already tells the agent not to echo those markers,
/// but if one leaks through we still must not show a raw QQ id to the chat
/// (no human reader can tell whose id that is). This is the defensive tail
/// of that contract:
///
/// - `@<bot>` is removed entirely.
/// - `@<QQ:1234>` (no nickname) is removed entirely.
/// - `@nickname<QQ:1234>` degrades to `@nickname` — the displayed name is
///   kept so the sentence still reads naturally, but the QQ id is dropped.
///
/// One trailing ASCII space after a removed placeholder is consumed so we
/// don't leave double spaces behind, and the result is trimmed. Text that
/// contains no placeholders is returned unchanged (aside from the trim).
fn sanitize_inbound_mentions(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut remainder = text;
    while !remainder.is_empty() {
        if remainder.starts_with('@') {
            if let Some((consumed, replacement)) = match_inbound_mention(remainder) {
                let replacement_empty = replacement.is_empty();
                out.push_str(&replacement);
                remainder = &remainder[consumed..];
                if replacement_empty {
                    if let Some(rest) = remainder.strip_prefix(' ') {
                        remainder = rest;
                    }
                }
                continue;
            }
        }
        let ch = remainder.chars().next().expect("non-empty remainder");
        out.push(ch);
        remainder = &remainder[ch.len_utf8()..];
    }
    out.trim().to_string()
}

/// Try to match one inbound mention placeholder at the start of `s`. On a hit
/// returns `(bytes_consumed, replacement)`; on a miss returns `None`.
fn match_inbound_mention(s: &str) -> Option<(usize, String)> {
    const BOT: &str = "@<bot>";
    if s.starts_with(BOT) {
        return Some((BOT.len(), String::new()));
    }

    if let Some(rest) = s.strip_prefix("@<QQ:") {
        let end = rest.find('>')?;
        let digits = &rest[..end];
        if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Some(("@<QQ:".len() + end + 1, String::new()));
        }
        return None;
    }

    let after_at = s.strip_prefix('@')?;
    let mut name_end = 0;
    loop {
        let tail = &after_at[name_end..];
        if tail.starts_with("<QQ:") {
            break;
        }
        let ch = tail.chars().next()?;
        if matches!(ch, '@' | '<' | '>') || ch.is_whitespace() {
            return None;
        }
        name_end += ch.len_utf8();
    }
    if name_end == 0 {
        return None;
    }
    let name = &after_at[..name_end];
    let after_tag = &after_at[name_end + "<QQ:".len()..];
    let gt_idx = after_tag.find('>')?;
    let digits = &after_tag[..gt_idx];
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let consumed = 1 + name_end + "<QQ:".len() + digits.len() + 1;
    Some((consumed, format!("@{name}")))
}

#[cfg(test)]
mod sanitize_tests {
    use super::sanitize_inbound_mentions;

    #[test]
    fn strips_leaked_raw_qq_placeholder() {
        let text = "@<QQ:1597226206> 起床了，看到消息回一下。";
        assert_eq!(sanitize_inbound_mentions(text), "起床了，看到消息回一下。");
    }

    #[test]
    fn keeps_nickname_when_metadata_leaks() {
        let text = "@suibianwanwan<QQ:1597226206> 不能帮你定向辱骂别人。";
        assert_eq!(
            sanitize_inbound_mentions(text),
            "@suibianwanwan 不能帮你定向辱骂别人。",
        );
    }

    #[test]
    fn strips_bot_self_marker() {
        let text = "@<bot> 处理完成。";
        assert_eq!(sanitize_inbound_mentions(text), "处理完成。");
    }

    #[test]
    fn handles_multiple_placeholders_in_one_line() {
        let text = "@<bot> 回复 @alice<QQ:111> 和 @<QQ:222> 的问题";
        assert_eq!(sanitize_inbound_mentions(text), "回复 @alice 和 的问题");
    }

    #[test]
    fn leaves_non_placeholder_at_text_alone() {
        let text = "看看 @example.com 和 user@host 这种普通文本";
        assert_eq!(sanitize_inbound_mentions(text), text);
    }

    #[test]
    fn rejects_empty_name_branch_without_qq_prefix() {
        let text = "@< not a placeholder";
        assert_eq!(sanitize_inbound_mentions(text), "@< not a placeholder");
    }

    #[test]
    fn rejects_malformed_qq_placeholder() {
        let text = "@<QQ:abc> still here";
        assert_eq!(sanitize_inbound_mentions(text), "@<QQ:abc> still here");
    }

    #[test]
    fn collapses_only_trailing_space_after_strip() {
        let text = "foo @<QQ:123>  bar";
        assert_eq!(sanitize_inbound_mentions(text), "foo  bar");
    }

    #[test]
    fn all_placeholders_reduces_to_empty_string() {
        let text = "   @<bot>  @<QQ:1>   ";
        assert_eq!(sanitize_inbound_mentions(text), "");
    }
}

#[cfg(test)]
mod build_outbound_tests {
    use std::path::PathBuf;

    use super::{
        build_outbound_message, OutboundSegment, OutboundTarget, ReplyPayload,
    };
    use crate::reply_context::ActiveReplyContext;

    fn group_context() -> ActiveReplyContext {
        ActiveReplyContext {
            token: "token".into(),
            conversation_key: "group:42".into(),
            is_group: true,
            reply_target_id: 42,
            source_message_id: 1000,
            source_sender_id: 7777,
            source_sender_name: "sender".into(),
            repo_root: PathBuf::from("/tmp"),
            artifacts_dir: PathBuf::from("/tmp/.run/artifacts"),
        }
    }

    fn private_context() -> ActiveReplyContext {
        ActiveReplyContext {
            is_group: false,
            reply_target_id: 123,
            ..group_context()
        }
    }

    fn reply_segments(message: &crate::outbound::OutboundMessage) -> Vec<i64> {
        message
            .segments
            .iter()
            .filter_map(|segment| match segment {
                OutboundSegment::Reply {
                    message_id,
                } => Some(*message_id),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn group_default_quotes_source_message_when_reply_to_absent() {
        let ctx = group_context();
        let message = build_outbound_message(
            &ctx,
            ReplyPayload::Text("hi".into()),
            &[],
            None,
        );
        assert_eq!(message.target, OutboundTarget::Group(42));
        assert_eq!(reply_segments(&message), vec![1000]);
    }

    #[test]
    fn group_custom_reply_to_overrides_default() {
        let ctx = group_context();
        let message = build_outbound_message(
            &ctx,
            ReplyPayload::Text("jump to old message".into()),
            &[],
            Some(55555),
        );
        assert_eq!(reply_segments(&message), vec![55555]);
    }

    #[test]
    fn private_target_carries_no_reply_or_at_segments() {
        let ctx = private_context();
        let message = build_outbound_message(
            &ctx,
            ReplyPayload::Text("privmsg".into()),
            &[],
            Some(9999),
        );
        assert_eq!(message.target, OutboundTarget::Private(123));
        assert!(reply_segments(&message).is_empty());
        assert!(!message
            .segments
            .iter()
            .any(|segment| matches!(segment, OutboundSegment::At { .. })));
    }

    #[test]
    fn group_preserves_at_targets_when_reply_to_set() {
        let ctx = group_context();
        let message = build_outbound_message(
            &ctx,
            ReplyPayload::Text("ok".into()),
            &[111, 222],
            Some(333),
        );
        assert_eq!(reply_segments(&message), vec![333]);
        let ats: Vec<i64> = message
            .segments
            .iter()
            .filter_map(|segment| match segment {
                OutboundSegment::At {
                    user_id,
                } => Some(*user_id),
                _ => None,
            })
            .collect();
        assert_eq!(ats, vec![111, 222]);
    }
}
