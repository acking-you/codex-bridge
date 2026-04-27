//! Runtime-pool executor for lane-based Codex execution.

use std::{collections::HashMap, fs, path::Path, sync::Arc};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::{
    codex_runtime::{
        ActiveTurn, CodexExecutor, CodexRuntime, CodexRuntimeConfig, CodexTurnResult,
        TurnProgressSink,
    },
    lane_manager::{RuntimeSlotSnapshot, RuntimeSlotState},
    runtime::runtime_slot_dir,
};

/// One app-server slot inside the runtime pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSlot {
    /// Stable slot identifier inside the pool.
    pub slot_id: usize,
    /// Current lifecycle state.
    pub state: RuntimeSlotState,
    /// Conversation currently assigned to this slot, when busy.
    pub assigned_conversation_key: Option<String>,
}

#[derive(Debug, Clone)]
struct LaneLease {
    slot_id: usize,
    thread_id: Option<String>,
}

struct RuntimeSlotHandle {
    executor: Arc<dyn CodexExecutor>,
}

#[derive(Debug)]
struct RuntimePoolState {
    slots: Vec<RuntimeSlot>,
    lane_leases: HashMap<String, LaneLease>,
    thread_slots: HashMap<String, usize>,
}

impl RuntimePoolState {
    fn new(size: usize) -> Self {
        Self {
            slots: (0..size)
                .map(|slot_id| RuntimeSlot {
                    slot_id,
                    state: RuntimeSlotState::Idle,
                    assigned_conversation_key: None,
                })
                .collect(),
            lane_leases: HashMap::new(),
            thread_slots: HashMap::new(),
        }
    }

    fn reserve_lane_slot(&mut self, conversation_key: &str) -> Result<usize> {
        if let Some(lease) = self.lane_leases.get(conversation_key) {
            return Ok(lease.slot_id);
        }
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| slot.state == RuntimeSlotState::Idle)
            .ok_or_else(|| anyhow!("no idle runtime slot available"))?;
        slot.state = RuntimeSlotState::Busy;
        slot.assigned_conversation_key = Some(conversation_key.to_string());
        self.lane_leases.insert(
            conversation_key.to_string(),
            LaneLease { slot_id: slot.slot_id, thread_id: None },
        );
        Ok(slot.slot_id)
    }

    fn bind_thread(
        &mut self,
        conversation_key: &str,
        previous_thread_id: Option<&str>,
        thread_id: &str,
    ) -> Result<()> {
        let lease = self
            .lane_leases
            .get_mut(conversation_key)
            .ok_or_else(|| anyhow!("missing runtime lease for conversation {conversation_key}"))?;
        if let Some(previous_thread_id) = previous_thread_id {
            self.thread_slots.remove(previous_thread_id);
        }
        if let Some(previous_thread_id) = lease.thread_id.replace(thread_id.to_string()) {
            self.thread_slots.remove(&previous_thread_id);
        }
        self.thread_slots
            .insert(thread_id.to_string(), lease.slot_id);
        Ok(())
    }

    fn release_lane(&mut self, conversation_key: &str) {
        let Some(lease) = self.lane_leases.remove(conversation_key) else {
            return;
        };
        if let Some(thread_id) = lease.thread_id {
            self.thread_slots.remove(&thread_id);
        }
        if let Some(slot) = self.slots.get_mut(lease.slot_id) {
            slot.state = RuntimeSlotState::Idle;
            slot.assigned_conversation_key = None;
        }
    }

    fn release_thread(&mut self, thread_id: &str) {
        let Some(slot_id) = self.thread_slots.remove(thread_id) else {
            return;
        };
        let conversation_key = self
            .lane_leases
            .iter()
            .find_map(|(conversation_key, lease)| {
                if lease.slot_id == slot_id && lease.thread_id.as_deref() == Some(thread_id) {
                    Some(conversation_key.clone())
                } else {
                    None
                }
            });
        if let Some(conversation_key) = conversation_key {
            self.release_lane(&conversation_key);
        }
    }

    fn reserve_ephemeral_slot(&mut self) -> Result<usize> {
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| slot.state == RuntimeSlotState::Idle)
            .ok_or_else(|| anyhow!("no idle runtime slot available"))?;
        slot.state = RuntimeSlotState::Busy;
        slot.assigned_conversation_key = None;
        Ok(slot.slot_id)
    }

    fn release_ephemeral_slot(&mut self, slot_id: usize) {
        if let Some(slot) = self.slots.get_mut(slot_id) {
            slot.state = RuntimeSlotState::Idle;
            slot.assigned_conversation_key = None;
        }
    }

    fn slot_snapshots(&self) -> Vec<RuntimeSlotSnapshot> {
        self.slots
            .iter()
            .map(|slot| RuntimeSlotSnapshot {
                slot_id: slot.slot_id,
                state: slot.state,
                assigned_conversation_key: slot.assigned_conversation_key.clone(),
            })
            .collect()
    }
}

/// Fixed-size pool of independent app-server executors sharing one Codex
/// thread store.
pub struct RuntimePool {
    slots: Vec<RuntimeSlotHandle>,
    state: Mutex<RuntimePoolState>,
}

impl RuntimePool {
    /// Build one runtime pool from pre-constructed per-slot executors.
    pub fn from_executors(executors: Vec<Arc<dyn CodexExecutor>>) -> Self {
        let slots = executors
            .into_iter()
            .map(|executor| RuntimeSlotHandle { executor })
            .collect::<Vec<_>>();
        Self { state: Mutex::new(RuntimePoolState::new(slots.len())), slots }
    }

    /// Spawn `size` concrete Codex runtimes that share the same `CODEX_HOME`
    /// but keep separate per-slot HOME directories.
    pub async fn spawn_from_config(
        base_config: &CodexRuntimeConfig,
        runtime_root: &Path,
        size: usize,
    ) -> Result<Self> {
        let mut executors = Vec::with_capacity(size);
        for slot_id in 0..size {
            let slot_root = runtime_slot_dir(runtime_root, slot_id);
            let slot_home = slot_root.join("home");
            fs::create_dir_all(&slot_home)?;

            let mut slot_config = base_config.clone();
            slot_config.child_home_root = slot_home;
            slot_config.client_name = format!("{}-slot-{slot_id}", slot_config.client_name);
            let runtime = CodexRuntime::new(slot_config).await?;
            executors.push(Arc::new(runtime) as Arc<dyn CodexExecutor>);
        }
        Ok(Self::from_executors(executors))
    }

    fn slot_executor(&self, slot_id: usize) -> Result<Arc<dyn CodexExecutor>> {
        self.slots
            .get(slot_id)
            .map(|slot| slot.executor.clone())
            .ok_or_else(|| anyhow!("runtime slot {slot_id} is out of range"))
    }

    async fn slot_id_for_thread(&self, thread_id: &str) -> Result<usize> {
        let state = self.state.lock().await;
        state
            .thread_slots
            .get(thread_id)
            .copied()
            .ok_or_else(|| anyhow!("thread {thread_id} is not leased to any runtime slot"))
    }
}

#[async_trait]
impl CodexExecutor for RuntimePool {
    async fn ensure_thread(
        &self,
        conversation_key: &str,
        existing_thread_id: Option<&str>,
    ) -> Result<String> {
        let slot_id = {
            let mut state = self.state.lock().await;
            state.reserve_lane_slot(conversation_key)?
        };
        let executor = self.slot_executor(slot_id)?;
        let thread_result = executor
            .ensure_thread(conversation_key, existing_thread_id)
            .await;

        let thread_id = match thread_result {
            Ok(thread_id) => thread_id,
            Err(error) => {
                let mut state = self.state.lock().await;
                state.release_lane(conversation_key);
                return Err(error);
            },
        };

        let mut state = self.state.lock().await;
        state.bind_thread(conversation_key, existing_thread_id, &thread_id)?;
        Ok(thread_id)
    }

    async fn start_turn(&self, thread_id: &str, input_text: &str) -> Result<ActiveTurn> {
        let slot_id = self.slot_id_for_thread(thread_id).await?;
        let executor = self.slot_executor(slot_id)?;
        executor.start_turn(thread_id, input_text).await
    }

    async fn wait_for_turn(&self, active_turn: &ActiveTurn) -> Result<CodexTurnResult> {
        self.wait_for_turn_with_progress(active_turn, None).await
    }

    async fn wait_for_turn_with_progress(
        &self,
        active_turn: &ActiveTurn,
        progress: Option<&dyn TurnProgressSink>,
    ) -> Result<CodexTurnResult> {
        let slot_id = self.slot_id_for_thread(&active_turn.thread_id).await?;
        let executor = self.slot_executor(slot_id)?;
        let result = executor
            .wait_for_turn_with_progress(active_turn, progress)
            .await;
        let mut state = self.state.lock().await;
        state.release_thread(&active_turn.thread_id);
        result
    }

    async fn interrupt(&self, thread_id: &str, turn_id: &str) -> Result<()> {
        let slot_id = self.slot_id_for_thread(thread_id).await?;
        let executor = self.slot_executor(slot_id)?;
        executor.interrupt(thread_id, turn_id).await
    }

    async fn compact_thread(&self, thread_id: &str) -> Result<()> {
        let mapped_slot_id = {
            let state = self.state.lock().await;
            state.thread_slots.get(thread_id).copied()
        };
        let (slot_id, ephemeral) = if let Some(slot_id) = mapped_slot_id {
            (slot_id, false)
        } else {
            let mut state = self.state.lock().await;
            (state.reserve_ephemeral_slot()?, true)
        };

        let executor = self.slot_executor(slot_id)?;
        let result = executor.compact_thread(thread_id).await;
        if ephemeral {
            let mut state = self.state.lock().await;
            state.release_ephemeral_slot(slot_id);
        }
        result
    }

    async fn runtime_slots(&self) -> Vec<RuntimeSlotSnapshot> {
        let state = self.state.lock().await;
        state.slot_snapshots()
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Mutex as StdMutex};

    use anyhow::Result;

    use super::*;

    #[derive(Debug)]
    struct FakeSlotExecutor {
        thread_ids: Mutex<VecDeque<String>>,
        calls: StdMutex<Vec<String>>,
    }

    impl FakeSlotExecutor {
        fn new(thread_ids: Vec<&str>) -> Self {
            Self {
                thread_ids: Mutex::new(
                    thread_ids
                        .into_iter()
                        .map(str::to_string)
                        .collect::<VecDeque<_>>(),
                ),
                calls: StdMutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().expect("calls").clone()
        }
    }

    #[async_trait]
    impl CodexExecutor for FakeSlotExecutor {
        async fn ensure_thread(
            &self,
            conversation_key: &str,
            _existing_thread_id: Option<&str>,
        ) -> Result<String> {
            self.calls
                .lock()
                .expect("calls")
                .push(format!("ensure:{conversation_key}"));
            self.thread_ids
                .lock()
                .await
                .pop_front()
                .ok_or_else(|| anyhow!("no thread id configured"))
        }

        async fn start_turn(&self, thread_id: &str, _input_text: &str) -> Result<ActiveTurn> {
            self.calls
                .lock()
                .expect("calls")
                .push(format!("start:{thread_id}"));
            Ok(ActiveTurn {
                thread_id: thread_id.to_string(),
                turn_id: format!("turn:{thread_id}"),
            })
        }

        async fn wait_for_turn(&self, active_turn: &ActiveTurn) -> Result<CodexTurnResult> {
            self.calls
                .lock()
                .expect("calls")
                .push(format!("wait:{}", active_turn.thread_id));
            Ok(CodexTurnResult {
                thread_id: active_turn.thread_id.clone(),
                turn_id: active_turn.turn_id.clone(),
                status: codex_app_server_protocol::TurnStatus::Completed,
                error_message: None,
                items: Vec::new(),
                final_reply: Some("ok".to_string()),
            })
        }

        async fn interrupt(&self, thread_id: &str, turn_id: &str) -> Result<()> {
            self.calls
                .lock()
                .expect("calls")
                .push(format!("interrupt:{thread_id}:{turn_id}"));
            Ok(())
        }

        async fn compact_thread(&self, thread_id: &str) -> Result<()> {
            self.calls
                .lock()
                .expect("calls")
                .push(format!("compact:{thread_id}"));
            Ok(())
        }
    }

    #[tokio::test]
    async fn runtime_pool_keeps_one_lane_on_one_slot_until_turn_completes() {
        let slot0 = Arc::new(FakeSlotExecutor::new(vec!["thread-a"]));
        let slot1 = Arc::new(FakeSlotExecutor::new(vec!["thread-b"]));
        let pool = RuntimePool::from_executors(vec![slot0.clone(), slot1.clone()]);

        let thread_id = pool
            .ensure_thread("group:1", None)
            .await
            .expect("ensure thread");
        let active_turn = pool.start_turn(&thread_id, "hi").await.expect("start turn");
        let _ = pool.wait_for_turn(&active_turn).await.expect("wait turn");

        assert_eq!(
            slot0.calls(),
            vec![
                "ensure:group:1".to_string(),
                "start:thread-a".to_string(),
                "wait:thread-a".to_string(),
            ]
        );
        assert!(slot1.calls().is_empty());
        assert_eq!(pool.runtime_slots().await.len(), 2);
        assert!(pool
            .runtime_slots()
            .await
            .iter()
            .all(|slot| slot.state == RuntimeSlotState::Idle));
    }

    #[tokio::test]
    async fn runtime_pool_rejects_second_lane_when_no_slot_is_idle() {
        let slot0 = Arc::new(FakeSlotExecutor::new(vec!["thread-a"]));
        let pool = RuntimePool::from_executors(vec![slot0]);

        let thread_id = pool
            .ensure_thread("group:1", None)
            .await
            .expect("ensure first thread");
        let _active_turn = pool.start_turn(&thread_id, "hi").await.expect("start turn");

        let error = pool
            .ensure_thread("group:2", None)
            .await
            .expect_err("pool should be full");
        assert!(error.to_string().contains("no idle runtime slot available"));
    }

    #[tokio::test]
    async fn runtime_pool_releases_slot_after_wait_completes() {
        let slot0 = Arc::new(FakeSlotExecutor::new(vec!["thread-a", "thread-b"]));
        let pool = RuntimePool::from_executors(vec![slot0]);

        let thread_a = pool
            .ensure_thread("group:1", None)
            .await
            .expect("ensure thread a");
        let active_a = pool.start_turn(&thread_a, "a").await.expect("start a");
        let _ = pool.wait_for_turn(&active_a).await.expect("wait a");

        let thread_b = pool
            .ensure_thread("group:2", None)
            .await
            .expect("ensure thread b");
        assert_eq!(thread_b, "thread-b");
    }
}
