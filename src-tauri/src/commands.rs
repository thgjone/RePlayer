//! v0 command surface: `start_replay` / `stop_replay` only, mirroring
//! `replay-engine`'s own Start/Stop-only scope. Pipeline wiring below is a
//! deliberate near-duplicate of `replay-cli::main` — see
//! `docs/tauri_v0_plan.md` for why that's an accepted v0 trade-off rather
//! than an extracted shared crate.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use format_pcap::PcapParser;
use output_tcp::{TcpClientOutput, TcpServerOutput, DEFAULT_RECONNECT_INTERVAL};
use output_udp::UdpOutput;
use replay_core::{
    Dispatcher, Output, OutputConfig, Parser as CoreParser, ReplaySpeed, Scenario, SourceId,
    TimelineEntry, TimelineManager,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tracing::{info, warn};

use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
struct StartedPayload {
    speed: f64,
}

#[derive(Debug, Clone, Serialize)]
struct PositionPayload {
    count: usize,
}

fn load_scenario(path: &Path) -> Result<Scenario, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("reading scenario file {}: {e}", path.display()))?;
    let scenario: Scenario = toml::from_str(&text)
        .map_err(|e| format!("parsing scenario file {}: {e}", path.display()))?;
    scenario
        .validate()
        .map_err(|e| format!("validating scenario file {}: {e}", path.display()))?;
    Ok(scenario)
}

/// One cursor per recording path, all sharing that recording's `SourceId`/
/// offset — `TimelineManager`'s merge already interleaves them correctly,
/// including multi-file recordings (see `docs/phase2_plan.md`).
fn load_timelines(scenario: &Scenario) -> Result<TimelineManager, String> {
    let mut timeline_manager = TimelineManager::new();
    for recording in &scenario.recordings {
        if recording.format != "pcap" {
            return Err(format!(
                "unsupported recording format '{}' (only 'pcap' is implemented)",
                recording.format
            ));
        }
        let source_id = SourceId::new(recording.id.clone());
        let offset = Duration::from_millis(recording.offset_ms);

        for path in &recording.paths {
            let file = File::open(path)
                .map_err(|e| format!("opening recording file {}: {e}", path.display()))?;
            let mut reader = BufReader::new(file);
            let timeline = PcapParser::new()
                .parse(&mut reader, source_id.clone())
                .map_err(|e| format!("parsing recording {}: {e}", path.display()))?;

            timeline_manager.add_timeline(TimelineEntry {
                source_id: source_id.clone(),
                timeline,
                offset,
            });
        }
    }
    Ok(timeline_manager)
}

async fn build_outputs(scenario: &Scenario) -> Result<HashMap<String, Arc<dyn Output>>, String> {
    let mut outputs: HashMap<String, Arc<dyn Output>> = HashMap::new();
    for output_config in &scenario.outputs {
        let output: Arc<dyn Output> = match output_config {
            OutputConfig::Udp {
                address,
                multicast_interface,
                ..
            } => {
                let udp = UdpOutput::bind(*address, *multicast_interface)
                    .map_err(|e| format!("binding UDP output to {address}: {e}"))?;
                Arc::new(udp)
            }
            OutputConfig::TcpServer { bind_address, .. } => {
                let server = TcpServerOutput::bind(*bind_address)
                    .await
                    .map_err(|e| format!("binding TCP server output to {bind_address}: {e}"))?;
                Arc::new(server)
            }
            OutputConfig::TcpClient { remote_address, .. } => {
                Arc::new(TcpClientOutput::new(*remote_address, DEFAULT_RECONNECT_INTERVAL))
            }
        };
        outputs.insert(output_config.id().to_string(), output);
    }
    Ok(outputs)
}

fn build_dispatcher(scenario: &Scenario, outputs: &HashMap<String, Arc<dyn Output>>) -> Dispatcher {
    let mut dispatcher = Dispatcher::new();
    for route in &scenario.routes {
        // Scenario::validate() already confirmed every route's ids exist.
        let output = outputs
            .get(&route.output_id)
            .expect("Scenario::validate already checked route.output_id exists");
        dispatcher.add_route(SourceId::new(route.recording_id.clone()), output.clone());
    }
    dispatcher
}

// Generic over `R: Runtime` (rather than the concrete `Wry`-backed
// `AppHandle`) so this can be exercised in tests against
// `tauri::test::MockRuntime` with no real window — see the `tests` module
// below.
#[tauri::command]
pub async fn start_replay<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    scenario_path: String,
) -> Result<(), String> {
    {
        let guard = state.0.lock().unwrap();
        if guard.is_some() {
            return Err("replay already running".to_string());
        }
    }

    let scenario = load_scenario(Path::new(&scenario_path))?;
    let timeline_manager = load_timelines(&scenario)?;
    let outputs = build_outputs(&scenario).await?;
    let dispatcher = build_dispatcher(&scenario, &outputs);
    let speed =
        ReplaySpeed::new(scenario.speed).expect("Scenario::validate already checked speed");

    let (handle, mut events) = replay_engine::spawn(Box::new(timeline_manager), speed);
    handle.start();
    *state.0.lock().unwrap() = Some(handle);

    info!(scenario = %scenario_path, speed = scenario.speed, "replay started");
    app.emit(
        "replay://started",
        StartedPayload {
            speed: scenario.speed,
        },
    )
    .map_err(|e| e.to_string())?;

    // Bridging task: drains the engine's event channel, dispatches each
    // event, and reports a dispatched-event count back to the frontend.
    // Elapsed *replay time* for a scrubber is deliberately NOT reported
    // here — see docs/tauri_v0_plan.md's note on why that's computed
    // client-side from wall-clock-since-`started` * speed instead.
    let bridge_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut count = 0usize;
        while let Some(event) = events.recv().await {
            dispatcher.dispatch(&event).await;
            count += 1;
            if let Err(err) = bridge_app.emit("replay://position", PositionPayload { count }) {
                warn!(%err, "failed to emit replay://position");
            }
        }
        *bridge_app.state::<AppState>().0.lock().unwrap() = None;
        info!(dispatched = count, "replay stopped");
        if let Err(err) = bridge_app.emit("replay://stopped", ()) {
            warn!(%err, "failed to emit replay://stopped");
        }
    });

    Ok(())
}

#[tauri::command]
pub async fn stop_replay(state: State<'_, AppState>) -> Result<(), String> {
    let guard = state.0.lock().unwrap();
    match guard.as_ref() {
        Some(handle) => {
            handle.stop();
            Ok(())
        }
        None => Err("no replay running".to_string()),
    }
}

/// Exercises the Tauri-specific glue (`AppState`, command guards, `emit`)
/// this module adds on top of `replay-cli`'s already-tested pipeline
/// wiring, using `tauri::test`'s `MockRuntime` — no real window needed.
/// Commands are generic over `R: Runtime` for exactly this reason.
#[cfg(test)]
mod tests {
    use super::*;
    use etherparse::PacketBuilder;
    use pcap_file::pcap::{PcapHeader, PcapPacket, PcapWriter};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration as StdDuration;
    use tauri::test::{mock_builder, mock_context, noop_assets};
    use tauri::Listener;
    use tokio::net::UdpSocket;
    use tokio::time::timeout;

    /// Single-packet synthetic pcap (see `format-pcap`'s tests for the same
    /// pattern) — a UDP payload of `b"hello"` at t=0, so replay_at is 0 and
    /// the event dispatches as soon as the engine starts, keeping the test
    /// fast and non-flaky.
    fn synthetic_pcap_path(dir: &std::path::Path) -> std::path::PathBuf {
        let mut buf = Vec::new();
        {
            let mut writer = PcapWriter::with_header(&mut buf, PcapHeader::default()).unwrap();
            let builder = PacketBuilder::ethernet2([0, 1, 2, 3, 4, 5], [6, 7, 8, 9, 10, 11])
                .ipv4([192, 168, 0, 1], [192, 168, 0, 2], 64)
                .udp(5000, 6000);
            let mut frame = Vec::new();
            builder.write(&mut frame, b"hello").unwrap();
            let packet = PcapPacket::new(StdDuration::from_secs(0), frame.len() as u32, &frame);
            writer.write_packet(&packet).unwrap();
        }
        let path = dir.join("fixture.pcap");
        std::fs::write(&path, &buf).unwrap();
        path
    }

    fn scenario_toml(pcap_path: &std::path::Path, udp_port: u16) -> String {
        format!(
            r#"
            [[recordings]]
            id = "rec"
            format = "pcap"
            paths = ["{pcap}"]

            [[outputs]]
            id = "udp-out"
            kind = "udp"
            address = "127.0.0.1:{udp_port}"

            [[routes]]
            recording_id = "rec"
            output_id = "udp-out"
            "#,
            pcap = pcap_path.display(),
        )
    }

    fn mock_app() -> tauri::App<tauri::test::MockRuntime> {
        mock_builder()
            .manage(AppState::default())
            .invoke_handler(tauri::generate_handler![start_replay, stop_replay])
            .build(mock_context(noop_assets()))
            .expect("failed to build mock tauri app")
    }

    #[tokio::test]
    async fn start_replay_dispatches_event_over_udp_and_emits_started() {
        let dir = tempfile::tempdir().unwrap();
        let listener = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let pcap_path = synthetic_pcap_path(dir.path());
        let scenario_path = dir.path().join("scenario.toml");
        std::fs::write(&scenario_path, scenario_toml(&pcap_path, port)).unwrap();

        let app = mock_app();
        let handle = app.handle().clone();

        let started_fired = Arc::new(AtomicBool::new(false));
        {
            let started_fired = started_fired.clone();
            handle.listen("replay://started", move |_event| {
                started_fired.store(true, Ordering::SeqCst);
            });
        }

        start_replay(
            handle.clone(),
            handle.state::<AppState>(),
            scenario_path.display().to_string(),
        )
        .await
        .expect("start_replay should succeed");

        let mut buf = [0u8; 64];
        let (len, _) = timeout(StdDuration::from_secs(2), listener.recv_from(&mut buf))
            .await
            .expect("timed out waiting for the UDP packet")
            .unwrap();
        assert_eq!(&buf[..len], b"hello");
        assert!(started_fired.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn start_replay_while_already_running_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let listener = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let pcap_path = synthetic_pcap_path(dir.path());
        let scenario_path = dir.path().join("scenario.toml");
        std::fs::write(&scenario_path, scenario_toml(&pcap_path, port)).unwrap();

        let app = mock_app();
        let handle = app.handle().clone();

        start_replay(
            handle.clone(),
            handle.state::<AppState>(),
            scenario_path.display().to_string(),
        )
        .await
        .expect("first start_replay should succeed");

        // AppState is populated synchronously before start_replay returns,
        // so no timing race: a second call right away must be rejected.
        let second = start_replay(
            handle.clone(),
            handle.state::<AppState>(),
            scenario_path.display().to_string(),
        )
        .await;
        assert_eq!(second, Err("replay already running".to_string()));

        stop_replay(handle.state::<AppState>()).await.unwrap();
    }

    #[tokio::test]
    async fn stop_replay_without_a_running_replay_errors() {
        let app = mock_app();
        let handle = app.handle().clone();

        let result = stop_replay(handle.state::<AppState>()).await;
        assert_eq!(result, Err("no replay running".to_string()));
    }
}
