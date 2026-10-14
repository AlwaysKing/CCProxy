// 本地代理服务：单端口接收所有协议请求，识别协议 → 查映射表 → 协议适配改写 → 转发上游。
// 配置通过 AppState 每请求现读，改配置即时生效，无需重启代理。

pub mod adapters;
pub mod detector;
pub mod fetch_models;
pub mod models;
pub mod router;
pub mod server;
pub mod upstream;

use std::sync::{Arc, OnceLock};

use tokio::net::TcpListener;
use tokio::sync::{oneshot, Mutex};

use crate::state::AppState;

struct Running {
    port: u16,
    shutdown: Option<oneshot::Sender<()>>,
    handle: tokio::task::JoinHandle<()>,
}

pub struct Proxy {
    ctx: OnceLock<Arc<AppState>>,
    inner: Mutex<Option<Running>>,
}

impl Default for Proxy {
    fn default() -> Self {
        Self::new()
    }
}

impl Proxy {
    pub fn new() -> Self {
        Self {
            ctx: OnceLock::new(),
            inner: Mutex::new(None),
        }
    }

    pub fn init(&self, ctx: Arc<AppState>) {
        let _ = self.ctx.set(ctx);
    }

    // 监听固定 127.0.0.1；端口占用时直接报错，不静默顺延（界面据此提示改端口）。
    // 生命周期操作共用一把锁：并发的 start/stop 自然串行，后到的 start 看到已监听
    // 就直接返回端口，stop 会等在途的 start 收尾完再关
    pub async fn start(&self) -> Result<u16, String> {
        let mut inner = self.inner.lock().await;
        if let Some(running) = inner.as_ref() {
            return Ok(running.port);
        }
        let Some(ctx) = self.ctx.get().cloned() else {
            return Err("代理未初始化".to_string());
        };
        let port = ctx.config.read().map(|config| config.settings.port).unwrap_or(crate::config::DEFAULT_PORT);

        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|error| describe_bind_error(&error, port))?;
        // 按实际绑到的端口回报：配置里写 0 时由系统挑一个（测试与诊断用）
        let port = listener.local_addr().map(|address| address.port()).unwrap_or(port);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server_state = ctx.clone();
        let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
            let state = server_state.clone();
            async move { server::handle(state, request).await }
        });
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });

        *inner = Some(Running { port, shutdown: Some(shutdown_tx), handle });
        Ok(port)
    }

    pub async fn stop(&self) {
        let mut inner = self.inner.lock().await;
        if let Some(mut running) = inner.take() {
            if let Some(shutdown) = running.shutdown.take() {
                let _ = shutdown.send(());
            }
            let _ = running.handle.await;
        }
    }

    pub fn running_port(&self) -> Option<u16> {
        self.inner.try_lock().ok().and_then(|inner| inner.as_ref().map(|running| running.port))
    }
}

fn describe_bind_error(error: &std::io::Error, port: u16) -> String {
    if error.kind() == std::io::ErrorKind::AddrInUse {
        format!("端口 {port} 已被占用，请在设置里换一个端口")
    } else {
        format!("无法监听 127.0.0.1:{port}：{error}")
    }
}

