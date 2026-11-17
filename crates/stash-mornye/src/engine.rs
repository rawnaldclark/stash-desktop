//! One running Mornye add-on: its own thread, QuickJS runtime and context, after the prelude ([`crate::PRELUDE`]).
//!
//! The add-on sees one synchronous `bridge(method, json)`. Each call is answered by the host on a thread of its own
//! while the add-on's thread waits; a callback the host runs meanwhile (a progress function the add-on passed it)
//! runs inside that wait, and may call the host again. Calls into the add-on are taken one at a time.
//!
//! Ported from Lokal's `rust/src/morneye.rs` (https://github.com/lokal-music/mobile, GPL-3.0), with a host that
//! answers synchronously in place of Dart's futures.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rquickjs::context::EvalOptions;
use rquickjs::{Context, Ctx, Exception, Function, Object, Persistent, Runtime, Value};

/// Answers one host call of the add-on: (method, JSON arguments) → JSON reply, `{"value": …}` or `{"failure": "…"}`.
pub type HostFn = Arc<dyn Fn(&str, &str) -> String + Send + Sync>;

const NO_DEADLINE: u64 = u64::MAX;
const STACK_LIMIT: usize = 512 * 1024;
/// How long loading an add-on may take.
const START_TIMEOUT_MS: u64 = 30_000;

/// The heap an add-on may use, from its config's `heapMb` (64 to 256 MiB).
fn memory_limit(config: &serde_json::Value) -> usize {
    let asked = config
        .get("heapMb")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(64);
    asked.clamp(64, 256) as usize * 1024 * 1024
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

enum Msg {
    Invoke {
        method: String,
        args: String,
        timeout_ms: u64,
        reply: mpsc::SyncSender<Result<String, String>>,
    },
    Reply {
        call: u64,
        json: String,
    },
    Callback {
        id: u64,
        args: String,
        ack: mpsc::SyncSender<String>,
    },
    Close,
}

/// One running add-on.
pub struct Engine {
    tx: Mutex<mpsc::Sender<Msg>>,
    closed: Arc<AtomicBool>,
    methods: Vec<String>,
}

impl Engine {
    /// Loads the package's `code` after the prelude with `config` (`permissions`, `signedSession`, `settings`,
    /// `rawFfmpeg`, `heapMb`). Fails when the package does not call `registerExtension()` or takes too long.
    pub fn start(code: &str, config: &serde_json::Value, host: HostFn) -> Result<Engine, String> {
        let (tx, rx) = mpsc::channel::<Msg>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<Vec<String>, String>>();
        let closed = Arc::new(AtomicBool::new(false));
        let thread_tx = tx.clone();
        let thread_closed = closed.clone();
        let code = code.to_string();
        let config = config.clone();
        std::thread::Builder::new()
            .name("mornye".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(move || run(code, config, host, rx, thread_tx, thread_closed, ready_tx))
            .map_err(|e| e.to_string())?;
        let methods = ready_rx
            .recv_timeout(Duration::from_millis(START_TIMEOUT_MS + 2_000))
            .unwrap_or_else(|_| Err("Extension loading timed out".to_string()));
        match methods {
            Ok(methods) => Ok(Engine {
                tx: Mutex::new(tx),
                closed,
                methods,
            }),
            Err(error) => {
                closed.store(true, Ordering::SeqCst);
                let _ = tx.send(Msg::Close);
                Err(error)
            }
        }
    }

    /// The functions the package registered.
    pub fn methods(&self) -> &[String] {
        &self.methods
    }

    /// Whether the package registered `name`.
    pub fn has(&self, name: &str) -> bool {
        self.methods.iter().any(|m| m == name)
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    fn send(&self, msg: Msg) -> bool {
        !self.is_closed()
            && self
                .tx
                .lock()
                .map(|tx| tx.send(msg).is_ok())
                .unwrap_or(false)
    }

    /// Calls the add-on's `method` with `args`; its result (JSON `null` for nothing). Waits at most `timeout` for the
    /// add-on's own work (a host call it waits on counts too); an add-on that runs past it is closed.
    pub fn invoke(
        &self,
        method: &str,
        args: &[serde_json::Value],
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        let (reply, answer) = mpsc::sync_channel(1);
        let msg = Msg::Invoke {
            method: method.to_string(),
            args: serde_json::Value::Array(args.to_vec()).to_string(),
            timeout_ms: timeout.as_millis().min(u64::MAX as u128) as u64,
            reply,
        };
        if !self.send(msg) {
            return Err("Extension runtime closed".into());
        }
        let json = answer
            .recv()
            .unwrap_or_else(|_| Err("Extension runtime closed".into()))?;
        Ok(serde_json::from_str(&json).unwrap_or(serde_json::Value::Null))
    }

    /// Runs the add-on's callback `id` (a function it passed to the host) while the host call it belongs to waits.
    /// The add-on's error message, empty when it succeeded.
    pub fn callback(&self, id: u64, args: &[serde_json::Value]) -> String {
        let (ack, answer) = mpsc::sync_channel(1);
        let msg = Msg::Callback {
            id,
            args: serde_json::Value::Array(args.to_vec()).to_string(),
            ack,
        };
        if !self.send(msg) {
            return "Extension operation cancelled".into();
        }
        answer
            .recv()
            .unwrap_or_else(|_| "Extension operation cancelled".into())
    }

    /// Stops the add-on; anything waiting on it fails.
    pub fn close(&self) {
        if !self.closed.swap(true, Ordering::SeqCst)
            && let Ok(tx) = self.tx.lock()
        {
            let _ = tx.send(Msg::Close);
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.close();
    }
}

/// What the add-on's bridge shares with the loop that runs invokes.
struct Shared {
    rx: mpsc::Receiver<Msg>,
    tx: mpsc::Sender<Msg>,
    host: HostFn,
    closed: Arc<AtomicBool>,
    deadline: Arc<AtomicU64>,
    next_call: RefCell<u64>,
    /// Invokes that came during a host call wait their turn.
    queued: RefCell<VecDeque<Msg>>,
    /// Replies to an outer call that came while an inner one waited.
    replies: RefCell<HashMap<u64, String>>,
    api: RefCell<Option<Persistent<Object<'static>>>>,
}

fn message(ctx: &Ctx<'_>, error: rquickjs::Error) -> String {
    let text = if let rquickjs::Error::Exception = error {
        let value = ctx.catch();
        if let Some(exception) = value.as_exception() {
            exception
                .message()
                .unwrap_or_else(|| "Extension error".into())
        } else if let Some(text) = value.as_string().and_then(|s| s.to_string().ok()) {
            text
        } else {
            "Extension error".into()
        }
    } else {
        error.to_string()
    };
    // QuickJS's interrupt comes out as an uncatchable "interrupted".
    if text.to_lowercase().contains("interrupted") {
        "Extension execution timed out".into()
    } else {
        text
    }
}

/// The add-on's synchronous view of a host call answered on another thread.
fn bridge(ctx: Ctx<'_>, shared: &Shared, method: String, json: String) -> rquickjs::Result<String> {
    let call = {
        let mut next = shared.next_call.borrow_mut();
        *next += 1;
        *next
    };
    let host = shared.host.clone();
    let tx = shared.tx.clone();
    std::thread::spawn(move || {
        let json = host(&method, &json);
        let _ = tx.send(Msg::Reply { call, json });
    });
    loop {
        if let Some(json) = shared.replies.borrow_mut().remove(&call) {
            return Ok(json);
        }
        if shared.closed.load(Ordering::SeqCst) {
            return Err(Exception::throw_message(
                &ctx,
                "Extension operation cancelled",
            ));
        }
        if now_ms() > shared.deadline.load(Ordering::SeqCst) {
            return Err(Exception::throw_message(
                &ctx,
                "Extension execution timed out",
            ));
        }
        match shared.rx.recv_timeout(Duration::from_millis(250)) {
            Ok(Msg::Reply { call: id, json }) if id == call => return Ok(json),
            Ok(Msg::Reply { call: id, json }) => {
                shared.replies.borrow_mut().insert(id, json);
            }
            Ok(Msg::Callback { id, args, ack }) => {
                let _ = ack.send(run_callback(&ctx, shared, id, args));
            }
            Ok(Msg::Close) => {
                shared.closed.store(true, Ordering::SeqCst);
                return Err(Exception::throw_message(
                    &ctx,
                    "Extension operation cancelled",
                ));
            }
            Ok(invoke @ Msg::Invoke { .. }) => shared.queued.borrow_mut().push_back(invoke),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(Exception::throw_message(
                    &ctx,
                    "Extension operation cancelled",
                ));
            }
        }
    }
}

fn api<'js>(ctx: &Ctx<'js>, shared: &Shared) -> rquickjs::Result<Object<'js>> {
    let saved = shared.api.borrow().clone();
    match saved {
        Some(api) => api.restore(ctx),
        None => Err(rquickjs::Error::new_from_js("undefined", "object")),
    }
}

fn run_callback(ctx: &Ctx<'_>, shared: &Shared, id: u64, args: String) -> String {
    let result = (|| -> rquickjs::Result<()> {
        let callback: Function = api(ctx, shared)?.get("callback")?;
        let value: Value = callback.call((id as f64, args))?;
        // A callback that returns a promise is settled before it is acknowledged.
        if let Some(promise) = value.as_promise() {
            promise.finish::<Value>()?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => String::new(),
        Err(error) => message(ctx, error),
    }
}

fn invoke(ctx: &Ctx<'_>, shared: &Shared, method: &str, args: &str) -> Result<String, String> {
    let result = (|| -> rquickjs::Result<String> {
        let invoke: Function = api(ctx, shared)?.get("invoke")?;
        let value: Value = invoke.call((method, args))?;
        let value = match value.as_promise() {
            Some(promise) => promise.finish::<Value>()?,
            None => value,
        };
        Ok(value
            .as_string()
            .map(|s| s.to_string())
            .transpose()?
            .unwrap_or_else(|| "null".into()))
    })();
    result.map_err(|error| match error {
        rquickjs::Error::WouldBlock => "Extension never finished its work".to_string(),
        error => message(ctx, error),
    })
}

fn run(
    code: String,
    config: serde_json::Value,
    host: HostFn,
    rx: mpsc::Receiver<Msg>,
    tx: mpsc::Sender<Msg>,
    closed: Arc<AtomicBool>,
    ready: mpsc::Sender<Result<Vec<String>, String>>,
) {
    let deadline = Arc::new(AtomicU64::new(now_ms() + START_TIMEOUT_MS));
    let setup = (|| -> Result<(Runtime, Context), String> {
        let runtime = Runtime::new().map_err(|e| e.to_string())?;
        runtime.set_memory_limit(memory_limit(&config));
        runtime.set_max_stack_size(STACK_LIMIT);
        let watch = deadline.clone();
        let stop = closed.clone();
        runtime.set_interrupt_handler(Some(Box::new(move || {
            stop.load(Ordering::SeqCst) || now_ms() > watch.load(Ordering::SeqCst)
        })));
        let context = Context::full(&runtime).map_err(|e| e.to_string())?;
        Ok((runtime, context))
    })();
    let (runtime, context) = match setup {
        Ok(pair) => pair,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let shared = Rc::new(Shared {
        rx,
        tx,
        host,
        closed: closed.clone(),
        deadline: deadline.clone(),
        next_call: RefCell::new(0),
        queued: RefCell::new(VecDeque::new()),
        replies: RefCell::new(HashMap::new()),
        api: RefCell::new(None),
    });

    let started = context.with(|ctx| -> Result<Vec<String>, String> {
        let guest = shared.clone();
        let bridge = Function::new(
            ctx.clone(),
            move |ctx: Ctx<'_>, method: String, json: String| bridge(ctx, &guest, method, json),
        )
        .map_err(|e| message(&ctx, e))?;
        let factory: Function = ctx.eval(crate::PRELUDE).map_err(|e| message(&ctx, e))?;
        let config = ctx
            .json_parse(config.to_string())
            .map_err(|e| message(&ctx, e))?;
        let api: Object = factory
            .call((bridge, config))
            .map_err(|e| message(&ctx, e))?;
        shared
            .api
            .replace(Some(Persistent::save(&ctx, api.clone())));
        let mut options = EvalOptions::default();
        options.global = true;
        options.strict = false;
        options.filename = Some("extension.js".into());
        ctx.eval_with_options::<Value, _>(code, options)
            .map_err(|e| message(&ctx, e))?;
        while ctx.execute_pending_job() {}
        let registered: Function = api.get("registered").map_err(|e| message(&ctx, e))?;
        if !registered
            .call::<_, bool>(())
            .map_err(|e| message(&ctx, e))?
        {
            return Err("Package did not call registerExtension()".into());
        }
        let methods: Function = api.get("methods").map_err(|e| message(&ctx, e))?;
        let json: String = methods.call(()).map_err(|e| message(&ctx, e))?;
        serde_json::from_str(&json).map_err(|e| e.to_string())
    });
    let failed = started.is_err();
    let _ = ready.send(started);
    deadline.store(NO_DEADLINE, Ordering::SeqCst);

    loop {
        if failed {
            break;
        }
        let next = shared.queued.borrow_mut().pop_front();
        let msg = match next {
            Some(msg) => msg,
            None => match shared.rx.recv() {
                Ok(msg) => msg,
                Err(_) => break,
            },
        };
        match msg {
            Msg::Invoke {
                method,
                args,
                timeout_ms,
                reply,
            } => {
                deadline.store(now_ms().saturating_add(timeout_ms.max(1)), Ordering::SeqCst);
                let result = context.with(|ctx| invoke(&ctx, &shared, &method, &args));
                deadline.store(NO_DEADLINE, Ordering::SeqCst);
                let timed_out =
                    matches!(&result, Err(e) if e.contains("timed out") || e.contains("cancelled"));
                let _ = reply.send(result);
                // An add-on stopped half-way is never used again.
                if timed_out || closed.load(Ordering::SeqCst) {
                    break;
                }
            }
            // Nothing waits on a host call: the callback is stale.
            Msg::Callback { ack, .. } => {
                let _ = ack.send(String::new());
            }
            Msg::Reply { .. } => {}
            Msg::Close => break,
        }
    }
    closed.store(true, Ordering::SeqCst);
    // Whatever was waiting behind the last invoke fails.
    for msg in shared.queued.borrow_mut().drain(..) {
        if let Msg::Invoke { reply, .. } = msg {
            let _ = reply.send(Err("Extension runtime closed".into()));
        }
    }
    while let Ok(msg) = shared.rx.try_recv() {
        if let Msg::Invoke { reply, .. } = msg {
            let _ = reply.send(Err("Extension runtime closed".into()));
        }
    }
    shared.api.replace(None);
    drop(shared);
    drop(context);
    runtime.run_gc();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config() -> serde_json::Value {
        json!({ "permissions": { "network": [], "storage": false, "file": false }, "settings": {} })
    }

    /// A host that echoes what it was asked.
    fn echo() -> HostFn {
        Arc::new(|method, args| {
            json!({ "value": { "method": method, "args": serde_json::from_str::<serde_json::Value>(args).unwrap() } })
                .to_string()
        })
    }

    fn start(code: &str, host: HostFn) -> Result<Engine, String> {
        Engine::start(code, &config(), host)
    }

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn the_heap_is_bounded_by_the_config() {
        assert_eq!(memory_limit(&json!({})), 64 << 20);
        assert_eq!(memory_limit(&json!({"heapMb": 256})), 256 << 20);
        assert_eq!(memory_limit(&json!({"heapMb": 5})), 64 << 20);
        assert_eq!(
            memory_limit(&json!({"heapMb": 999_999_999_999u64})),
            256 << 20
        );
    }

    #[test]
    fn a_package_registers_and_is_asked() {
        let engine = start(
            "registerExtension({ add: (a, b) => a + b, hi() { return 'x' } })",
            echo(),
        )
        .unwrap();
        assert_eq!(engine.methods(), ["add", "hi"]);
        assert_eq!(
            engine.invoke("add", &[json!(2), json!(3)], SECOND).unwrap(),
            json!(5)
        );
        // What it lacks is nothing.
        assert_eq!(engine.invoke("absent", &[], SECOND).unwrap(), json!(null));
    }

    #[test]
    fn a_package_that_registers_nothing_is_refused() {
        let error = start("1 + 1", echo()).err().unwrap();
        assert!(error.contains("registerExtension"), "{error}");
    }

    #[test]
    fn host_calls_are_synchronous_for_the_package() {
        let engine = start(
            "registerExtension({ ask: () => matching.normalizeString('X') })",
            echo(),
        )
        .unwrap();
        let said = engine.invoke("ask", &[], SECOND).unwrap();
        assert_eq!(said["method"], "matching.normalizeString");
        assert_eq!(said["args"], json!(["X"]));
    }

    #[test]
    fn promises_settle_and_errors_carry_their_message() {
        let engine = start(
            "registerExtension({ later: async () => { await null; return 7 }, boom: () => { throw new Error('no track') } })",
            echo(),
        )
        .unwrap();
        assert_eq!(engine.invoke("later", &[], SECOND).unwrap(), json!(7));
        assert_eq!(engine.invoke("boom", &[], SECOND).unwrap_err(), "no track");
        // A thrown error leaves the add-on usable.
        assert!(!engine.is_closed());
    }

    #[test]
    fn a_failing_host_call_throws_in_the_package() {
        let host: HostFn =
            Arc::new(|_, _| json!({ "failure": "Storage permission denied" }).to_string());
        let engine = start("registerExtension({ ask: () => { try { utils.md5('x') } catch (e) { return e.message } } })", host).unwrap();
        assert_eq!(
            engine.invoke("ask", &[], SECOND).unwrap(),
            json!("Storage permission denied")
        );
    }

    #[test]
    fn runaway_code_times_out_and_closes() {
        let engine = start("registerExtension({ spin: () => { for (;;) {} } })", echo()).unwrap();
        let error = engine
            .invoke("spin", &[], Duration::from_millis(200))
            .unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            engine
                .invoke("spin", &[], Duration::from_millis(200))
                .is_err()
        );
    }

    #[test]
    fn callbacks_run_during_a_host_call_and_may_call_the_host() {
        // The host answers `progress` only after running the add-on's function, which calls the host itself.
        let slot: Arc<Mutex<Option<Arc<Engine>>>> = Arc::default();
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let host: HostFn = {
            let (slot, seen) = (slot.clone(), seen.clone());
            Arc::new(move |method, args| {
                seen.lock().unwrap().push(method.to_string());
                if method == "file.download" {
                    let args: serde_json::Value = serde_json::from_str(args).unwrap();
                    let id = args[2]["onProgress"]["__callback"].as_u64().unwrap();
                    let engine = loop {
                        if let Some(engine) = slot.lock().unwrap().clone() {
                            break engine;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    };
                    assert_eq!(engine.callback(id, &[json!(50), json!(100)]), "");
                    return json!({ "value": "done" }).to_string();
                }
                json!({ "value": true }).to_string()
            })
        };
        let mut config = config();
        config["permissions"]["file"] = json!(true);
        let engine = Arc::new(
            Engine::start(
                "registerExtension({ get: () => { let p = 0; const r = file.download('u', 'f', { onProgress: (w, t) => { p = w; log.info(w); } }); return r + ':' + p } })",
                &config,
                host,
            )
            .unwrap(),
        );
        *slot.lock().unwrap() = Some(engine.clone());
        assert_eq!(
            engine.invoke("get", &[], Duration::from_secs(5)).unwrap(),
            json!("done:50")
        );
        assert_eq!(*seen.lock().unwrap(), ["file.download", "log"]);
        engine.close();
    }
}
