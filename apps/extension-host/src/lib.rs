// SPDX-License-Identifier: MPL-2.0
//! Only this optional process links Wasmtime/WASI. Each invocation owns a fresh
//! store and an operation-scoped watchdog; no timer survives an idle invocation.
use bareline_extensions_protocol::MEMORY_LIMIT;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

type Broker = Box<dyn FnMut(Vec<u8>) -> Result<Vec<u8>, String> + Send>;
struct State {
    invocation: Vec<u8>,
    broker: Broker,
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
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
        self.invoke_with_broker(
            bytes,
            vec![],
            Box::new(|_| Err("no broker grant".into())),
            cancelled,
        )
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
    fn invoke_budget(
        &self,
        bytes: &[u8],
        cancelled: Arc<AtomicBool>,
        budget: Duration,
    ) -> wasmtime::Result<()> {
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
        if cancelled.load(Ordering::Acquire) {
            wasmtime::bail!("cancelled");
        }
        if invocation.len() > bareline_extensions_protocol::MAX_CHUNK_BYTES {
            wasmtime::bail!("invocation limit");
        }
        if bytes.len() > 32 * 1024 * 1024 {
            wasmtime::bail!("component size exceeds 32 MiB");
        }

        let mut wasi = WasiCtxBuilder::new();
        wasi.allow_tcp(false)
            .allow_udp(false)
            .allow_ip_name_lookup(false);
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
        };
        let mut store = Store::new(&self.engine, state);
        store.limiter(|state| &mut state.limits);
        store.set_fuel(fuel)?;
        store.set_epoch_deadline(1);
        let mut linker = Linker::new(&self.engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        let mut api = linker.instance("bareline:extension/broker@1.0.0")?;
        api.func_wrap(
            "invocation",
            |store: wasmtime::StoreContextMut<State>, (): ()| {
                Ok((store.data().invocation.clone(),))
            },
        )?;
        api.func_wrap(
            "request",
            |mut store: wasmtime::StoreContextMut<State>, (frame,): (Vec<u8>,)| {
                if frame.len() > bareline_extensions_protocol::MAX_FRAME_BYTES {
                    return Ok((Err::<Vec<u8>, String>("frame limit".into()),));
                }
                let result = (store.data_mut().broker)(frame);
                Ok((result.and_then(|bytes| {
                    if bytes.len() <= bareline_extensions_protocol::MAX_CHUNK_BYTES {
                        Ok(bytes)
                    } else {
                        Err("reply chunk limit".into())
                    }
                }),))
            },
        )?;
        let engine = self.engine.clone();
        let (stop_tx, stop_rx) = mpsc::channel();
        // Fine-grained cancellation without an always-running host timer.
        let watchdog = std::thread::spawn(move || {
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
        });
        let result = (|| {
            let component = Component::new(&self.engine, bytes)?;
            let instance = linker.instantiate(&mut store, &component)?;
            let run = instance.get_typed_func::<(), ()>(&mut store, "run")?;
            run.call(&mut store, ())?;
            Ok(())
        })();
        let _ = stop_tx.send(());
        let _ = watchdog.join();
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    const SAFE: &str = "(component (core module $m (func (export \"run\"))) (core instance $i (instantiate $m)) (func (export \"run\") (canon lift (core func $i \"run\"))))";
    const LOOP: &str = "(component (core module $m (func (export \"run\") (loop $l br $l))) (core instance $i (instantiate $m)) (func (export \"run\") (canon lift (core func $i \"run\"))))";
    #[test]
    fn safe_component_runs_and_cpu_loop_terminates() {
        let rt = Runtime::new().unwrap();
        rt.invoke(SAFE.as_bytes(), Arc::new(AtomicBool::new(false)))
            .unwrap();
        let now = std::time::Instant::now();
        assert!(
            rt.invoke_budget(
                LOOP.as_bytes(),
                Arc::new(AtomicBool::new(false)),
                Duration::from_millis(50)
            )
            .is_err()
        );
        assert!(now.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn oversized_memory_rejected_and_malformed_component_isolated() {
        let rt = Runtime::new().unwrap();
        let large = "(component (core module $m (memory 2049) (func (export \"run\"))) (core instance $i (instantiate $m)) (func (export \"run\") (canon lift (core func $i \"run\"))))";
        assert!(
            rt.invoke(large.as_bytes(), Arc::new(AtomicBool::new(false)))
                .is_err()
        );
        assert!(
            rt.invoke(b"garbage", Arc::new(AtomicBool::new(false)))
                .is_err()
        );
        rt.invoke(SAFE.as_bytes(), Arc::new(AtomicBool::new(false)))
            .unwrap();
    }
}
