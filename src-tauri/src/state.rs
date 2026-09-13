use std::sync::Mutex;

use replay_engine::ReplayEngineHandle;

/// v0: a single global replay slot. Starting while one is already running
/// is a command error, not a second concurrent engine — see
/// `docs/tauri_v0_plan.md`.
#[derive(Default)]
pub struct AppState(pub Mutex<Option<ReplayEngineHandle>>);
