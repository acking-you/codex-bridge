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
}

/// Allowed reply payload forms.
#[allow(missing_docs)]
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
#[allow(missing_docs)]
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
    /// Convert one API/CLI request into a validated reply payload.
    pub fn into_payload(self, context: &ActiveReplyContext) -> Result<ReplyPayload> {
        let Self {
            token: _,
            text,
            image,
            file,
        } = self;
        let variants = usize::from(text.is_some())
            + usize::from(image.is_some())
            + usize::from(file.is_some());
        if variants != 1 {
            bail!("reply request must contain exactly one of text, image, or file");
        }

        if let Some(text) = text {
            let text = text.trim().to_string();
            if text.is_empty() {
                bail!("reply text must not be empty");
            }
            return Ok(ReplyPayload::Text(text));
        }

        if let Some(path) = image {
            return Ok(ReplyPayload::Image {
                path: resolve_artifact_path(&path, context)?,
            });
        }

        let path =
            resolve_artifact_path(file.as_ref().expect("checked single payload variant"), context)?;
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            bail!("reply file name is invalid");
        };
        let name = name.to_string();
        Ok(ReplyPayload::File {
            path,
            name,
        })
    }
}

/// Build a transport-ready outbound message from the active context.
pub fn build_outbound_message(
    context: &ActiveReplyContext,
    payload: ReplyPayload,
) -> OutboundMessage {
    let target = if context.is_group {
        OutboundTarget::Group(context.reply_target_id)
    } else {
        OutboundTarget::Private(context.reply_target_id)
    };

    let mut segments = Vec::new();
    if context.is_group {
        segments.push(OutboundSegment::Reply {
            message_id: context.source_message_id,
        });
        segments.push(OutboundSegment::At {
            user_id: context.source_sender_id,
        });
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
