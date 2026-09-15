// SPDX-License-Identifier: MPL-2.0
//! Only this optional process links Wasmtime/WASI. Each invocation owns a fresh
//! store and an operation-scoped watchdog; no timer survives an idle invocation.
use bareline_extensions_protocol::MEMORY_LIMIT;
use std::io::Write;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};
use std::time::Duration;
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

type Broker = Box<dyn FnMut(Vec<u8>) -> Result<Vec<u8>, String> + Send>;
type WatchdogRun = Box<dyn FnOnce() + Send + 'static>;
struct State {
    invocation: Vec<u8>,
    broker: Broker,
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
    qualification_telemetry: bool,
    execution_started: std::time::Instant,
    first_broker_reported: bool,
    response_reported: bool,
}

const MAX_QUALIFICATION_TELEMETRY_BYTES: usize = 64 * 1024;
static QUALIFICATION_TELEMETRY_FILE: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();
static QUALIFICATION_TELEMETRY_BYTES: AtomicUsize = AtomicUsize::new(0);

fn qualification_telemetry() -> bool {
    std::env::var_os("BARELINE_QUALIFICATION_TELEMETRY").is_some_and(|value| value == "1")
}

fn qualification_case_id() -> String {
    std::env::var("BARELINE_QUALIFICATION_CASE_ID").unwrap_or_else(|_| "uncorrelated".into())
}

/// Write one bounded qualification event to the parent's explicitly granted receipt.
/// Production launches do not set either qualification environment variable.
#[doc(hidden)]
pub fn report_qualification_event(event: String) {
    if !qualification_telemetry() {
        return;
    }
    let event = format!("{event}\n");
    if event.len() > 4096
        || QUALIFICATION_TELEMETRY_BYTES.fetch_add(event.len(), Ordering::Relaxed) + event.len()
            > MAX_QUALIFICATION_TELEMETRY_BYTES
    {
        return;
    }
    let sink = QUALIFICATION_TELEMETRY_FILE.get_or_init(|| {
        std::env::var_os("BARELINE_QUALIFICATION_TELEMETRY_PATH").and_then(|path| {
            let mut options = std::fs::OpenOptions::new();
            options.append(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.access_mode(0x0000_0004);
            }
            options.open(path).ok().map(Mutex::new)
        })
    });
    if let Some(sink) = sink
        && let Ok(mut file) = sink.lock()
        && file.write_all(event.as_bytes()).and_then(|()| file.flush()).is_ok()
    {
        return;
    }
    eprint!("{event}");
}

fn report_phase(phase: &str, elapsed: Duration) {
    let case_id = qualification_case_id();
    let host_pid = std::process::id();
    report_qualification_event(format!(
        "{{\"schema\":\"bareline.first-party-runtime.v1\",\"event\":\"phase\",\"case_id\":\"{case_id}\",\"host_pid\":{host_pid},\"phase\":\"{phase}\",\"elapsed_us\":{}}}",
        elapsed.as_micros()
    ));
}

fn report_phase_start(phase: &str) {
    let case_id = qualification_case_id();
    let host_pid = std::process::id();
    report_qualification_event(format!(
        "{{\"schema\":\"bareline.first-party-runtime.v1\",\"event\":\"phase_start\",\"case_id\":\"{case_id}\",\"host_pid\":{host_pid},\"phase\":\"{phase}\"}}"
    ));
}
impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}
pub struct Runtime {
    engine: Engine,
}
impl Runtime {
    pub fn new() -> wasmtime::Result<Self> {
        let mut config = Config::new();
        config
            .wasm_component_model(true)
            .cranelift_opt_level(wasmtime::OptLevel::None)
            .epoch_interruption(true)
            .consume_fuel(true);
        Ok(Self {
            engine: Engine::new(&config)?,
        })
    }
    /// Only bytes from a verified installed package may be supplied by the parent.
    /// No inherited directory, env, stdio, network, or process authority is exposed.
    pub fn invoke(&self, bytes: &[u8], cancelled: Arc<AtomicBool>) -> wasmtime::Result<()> {
        self.invoke_with_broker(bytes, vec![], Box::new(|_| Err("no broker grant".into())), cancelled)
    }
    pub fn invoke_with_broker(
        &self,
        bytes: &[u8],
        invocation: Vec<u8>,
        broker: Broker,
        cancelled: Arc<AtomicBool>,
    ) -> wasmtime::Result<()> {
        self.invoke_with_policy(
            bytes,
            invocation,
            broker,
            cancelled,
            bareline_extensions_protocol::ExecutionBudget::Interactive,
        )
    }
    pub fn invoke_with_policy(
        &self,
        bytes: &[u8],
        invocation: Vec<u8>,
        broker: Broker,
        cancelled: Arc<AtomicBool>,
        policy: bareline_extensions_protocol::ExecutionBudget,
    ) -> wasmtime::Result<()> {
        self.execute(
            bytes,
            invocation,
            broker,
            cancelled,
            Duration::from_millis(policy.timeout_ms()),
            policy.fuel(),
        )
    }
    #[cfg(test)]
    fn invoke_budget(&self, bytes: &[u8], cancelled: Arc<AtomicBool>, budget: Duration) -> wasmtime::Result<()> {
        self.execute(
            bytes,
            vec![],
            Box::new(|_| Err("denied".into())),
            cancelled,
            budget,
            50_000_000,
        )
    }
    fn execute(
        &self,
        bytes: &[u8],
        invocation: Vec<u8>,
        broker: Broker,
        cancelled: Arc<AtomicBool>,
        budget: Duration,
        fuel: u64,
    ) -> wasmtime::Result<()> {
        self.execute_with_watchdog_spawner(bytes, invocation, broker, cancelled, budget, fuel, |name, run| {
            std::thread::Builder::new().name(name).spawn(run)
        })
    }

    fn execute_with_watchdog_spawner(
        &self,
        bytes: &[u8],
        invocation: Vec<u8>,
        broker: Broker,
        cancelled: Arc<AtomicBool>,
        budget: Duration,
        fuel: u64,
        spawn_watchdog: impl FnOnce(String, WatchdogRun) -> std::io::Result<std::thread::JoinHandle<()>>,
    ) -> wasmtime::Result<()> {
        if cancelled.load(Ordering::Acquire) {
            wasmtime::bail!("cancelled");
        }
        if invocation.len() > bareline_extensions_protocol::MAX_CHUNK_BYTES {
            wasmtime::bail!("invocation limit");
        }
        if bytes.len() > 32 * 1024 * 1024 {
            wasmtime::bail!("component size exceeds 32 MiB");
        }

        let telemetry = qualification_telemetry();
        let prepare_started = std::time::Instant::now();
        if telemetry {
            report_phase_start("prepare");
        }
        let mut wasi = WasiCtxBuilder::new();
        wasi.allow_tcp(false).allow_udp(false).allow_ip_name_lookup(false);
        let state = State {
            invocation,
            broker,
            wasi: wasi.build(),
            table: ResourceTable::new(),
            limits: StoreLimitsBuilder::new()
                .memory_size(MEMORY_LIMIT)
                .memories(1)
                .tables(8)
                .table_elements(65536)
                .instances(16)
                .trap_on_grow_failure(true)
                .build(),
            qualification_telemetry: telemetry,
            execution_started: std::time::Instant::now(),
            first_broker_reported: false,
            response_reported: false,
        };
        let mut store = Store::new(&self.engine, state);
        store.limiter(|state| &mut state.limits);
        store.set_fuel(fuel)?;
        store.set_epoch_deadline(1);
        let mut linker = Linker::new(&self.engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        let mut api = linker.instance("bareline:extension/broker@1.0.0")?;
        api.func_wrap("invocation", |store: wasmtime::StoreContextMut<State>, (): ()| {
            Ok((store.data().invocation.clone(),))
        })?;
        api.func_wrap(
            "request",
            |mut store: wasmtime::StoreContextMut<State>, (frame,): (Vec<u8>,)| {
                if frame.len() > bareline_extensions_protocol::MAX_FRAME_BYTES {
                    return Ok((Err::<Vec<u8>, String>("frame limit".into()),));
                }
                if store.data().qualification_telemetry && !store.data().first_broker_reported {
                    report_phase("first_broker", store.data().execution_started.elapsed());
                    store.data_mut().first_broker_reported = true;
                }
                let report_response = store.data().qualification_telemetry && !store.data().response_reported;
                let response_started = std::time::Instant::now();
                if report_response {
                    report_phase_start("response");
                }
                let result = (store.data_mut().broker)(frame);
                if report_response {
                    report_phase("response", response_started.elapsed());
                    store.data_mut().response_reported = true;
                }
                Ok((result.and_then(|bytes| {
                    if bytes.len() <= bareline_extensions_protocol::MAX_CHUNK_BYTES {
                        Ok(bytes)
                    } else {
                        Err("reply chunk limit".into())
                    }
                }),))
            },
        )?;
        if telemetry {
            report_phase("prepare", prepare_started.elapsed());
        }
        let engine = self.engine.clone();
        let (stop_tx, stop_rx) = mpsc::channel();
        // Fine-grained cancellation without an always-running host timer.
        let watchdog = spawn_watchdog(
            "bareline-extension-host-watchdog".into(),
            Box::new(move || {
                let deadline = std::time::Instant::now() + budget;
                loop {
                    if cancelled.load(Ordering::Acquire) || std::time::Instant::now() >= deadline {
                        engine.increment_epoch();
                        if stop_rx.recv_timeout(Duration::from_millis(50)).is_err() {
                            std::process::exit(124);
                        }
                        return;
                    }
                    match stop_rx.recv_timeout(Duration::from_millis(10)) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
            }),
        )
        .map_err(|error| wasmtime::Error::msg(format!("extension watchdog unavailable: {error}")))?;
        let result: wasmtime::Result<()> = (|| {
            let compile_started = std::time::Instant::now();
            if store.data().qualification_telemetry {
                report_phase_start("compile");
            }
            let component = Component::from_binary(&self.engine, bytes)?;
            if store.data().qualification_telemetry {
                report_phase("compile", compile_started.elapsed());
            }
            let instantiate_started = std::time::Instant::now();
            if store.data().qualification_telemetry {
                report_phase_start("instantiate");
            }
            let instance = linker.instantiate(&mut store, &component)?;
            let run = instance.get_typed_func::<(), ()>(&mut store, "run")?;
            if store.data().qualification_telemetry {
                report_phase("instantiate", instantiate_started.elapsed());
            }
            store.data_mut().execution_started = std::time::Instant::now();
            let execute_started = std::time::Instant::now();
            if store.data().qualification_telemetry {
                report_phase_start("execute");
            }
            run.call(&mut store, ())?;
            if store.data().qualification_telemetry {
                report_phase("execute", execute_started.elapsed());
            }
            Ok(())
        })();
        let _ = stop_tx.send(());
        if watchdog.join().is_err() {
            return Err(wasmtime::Error::msg("extension watchdog stopped"));
        }
        result.map_err(|error| {
            let diagnostic = format!(
                "extension execution failed: trap={:?}, remaining_fuel={:?}",
                error.downcast_ref::<wasmtime::Trap>(),
                store.get_fuel(),
            );
            error.context(diagnostic)
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    const SAFE: &str = "(component (core module $m (func (export \"run\"))) (core instance $i (instantiate $m)) (func (export \"run\") (canon lift (core func $i \"run\"))))";
    const LOOP: &str = "(component (core module $m (func (export \"run\") (loop $l br $l))) (core instance $i (instantiate $m)) (func (export \"run\") (canon lift (core func $i \"run\"))))";
    // SEC-08 switched the runtime to `Component::from_binary`, so fixtures must be
    // assembled to a binary component here (the `wat` dev-dependency), matching
    // tests/isolation.rs. Text that is not valid WAT is passed through unchanged
    // so malformed-binary isolation is still exercised.
    fn asm(component: &str) -> Vec<u8> {
        wat::parse_str(component).unwrap_or_else(|_| component.as_bytes().to_vec())
    }
    #[test]
    fn safe_component_runs_and_cpu_loop_terminates() {
        let rt = Runtime::new().unwrap();
        rt.invoke(&asm(SAFE), Arc::new(AtomicBool::new(false))).unwrap();
        let now = std::time::Instant::now();
        assert!(
            rt.invoke_budget(&asm(LOOP), Arc::new(AtomicBool::new(false)), Duration::from_millis(50))
                .is_err()
        );
        assert!(now.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn oversized_memory_rejected_and_malformed_component_isolated() {
        let rt = Runtime::new().unwrap();
        let large = "(component (core module $m (memory 2049) (func (export \"run\"))) (core instance $i (instantiate $m)) (func (export \"run\") (canon lift (core func $i \"run\"))))";
        assert!(rt.invoke(&asm(large), Arc::new(AtomicBool::new(false))).is_err());
        assert!(rt.invoke(b"garbage", Arc::new(AtomicBool::new(false))).is_err());
        rt.invoke(&asm(SAFE), Arc::new(AtomicBool::new(false))).unwrap();
    }
    #[test]
    fn watchdog_spawn_failure_precedes_component_execution() {
        let rt = Runtime::new().unwrap();
        let error = rt
            .execute_with_watchdog_spawner(
                b"not a component",
                vec![],
                Box::new(|_| panic!("broker must not run")),
                Arc::new(AtomicBool::new(false)),
                Duration::from_secs(1),
                1,
                |_name, _run| Err(std::io::Error::other("controlled watchdog spawn failure")),
            )
            .unwrap_err();
        assert!(error.to_string().contains("extension watchdog unavailable"));
    }
}
