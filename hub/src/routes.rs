use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post, put};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::state::HubState;
use crate::{api, auth, blob, device, health, history, relay, web};

const MAX_JSON_BYTES: usize = 1024 * 1024;
const MAX_CHUNK_BYTES: usize = 2 * 1024 * 1024;

pub type HubRouteFn = fn(Router<Arc<HubState>>) -> Router<Arc<HubState>>;

/// 插件在 mount 时贡献路由；Host 按 boot order 组装。
pub struct HubRouter {
    contribs: Mutex<Vec<(&'static str, HubRouteFn)>>,
}

impl HubRouter {
    pub fn new() -> Self {
        Self { contribs: Mutex::new(Vec::new()) }
    }

    pub fn contribute(&self, id: &'static str, build: HubRouteFn) {
        self.contribs.lock().expect("hub router").push((id, build));
    }

    pub fn finish(&self, order: &[&str], state: Arc<HubState>) -> Router {
        let contribs = self.contribs.lock().expect("hub router").clone();
        let mut app = Router::<Arc<HubState>>::new();
        for id in order {
            if let Some((_, build)) = contribs.iter().find(|(plugin, _)| *plugin == *id) {
                app = build(app);
            }
        }
        app.layer(TraceLayer::new_for_http())
            .layer(RequestBodyLimitLayer::new(MAX_CHUNK_BYTES))
            .layer(DefaultBodyLimit::max(MAX_JSON_BYTES.max(MAX_CHUNK_BYTES)))
            .with_state(state)
    }
}

pub fn maintenance_routes(app: Router<Arc<HubState>>) -> Router<Arc<HubState>> {
    app.route("/healthz", get(health::healthz)).route("/readyz", get(health::readyz))
}

pub fn auth_routes(app: Router<Arc<HubState>>) -> Router<Arc<HubState>> {
    app.route("/api/v1/auth/session", post(auth::session))
        .route("/api/v1/pairing/start", post(auth::pairing_start))
        .route("/api/v1/pairing/finish", post(auth::pairing_finish))
        .route("/api/v1/pairing/avk", post(auth::deposit_avk))
}

pub fn device_routes(app: Router<Arc<HubState>>) -> Router<Arc<HubState>> {
    app.route("/api/v1/devices", get(device::list))
        .route("/api/v1/devices/{id}", delete(device::revoke))
}

pub fn history_routes(app: Router<Arc<HubState>>) -> Router<Arc<HubState>> {
    app.route("/api/v1/history", get(history::list).post(history::create))
        .route("/api/v1/history/{id}", delete(history::delete))
}

pub fn blob_routes(app: Router<Arc<HubState>>) -> Router<Arc<HubState>> {
    app.route("/api/v1/blobs", post(blob::begin))
        .route("/api/v1/blobs/{id}/chunks/{index}", put(blob::put_chunk).get(blob::get_chunk))
        .route("/api/v1/blobs/{id}/commit", post(blob::commit))
}

pub fn relay_routes(app: Router<Arc<HubState>>) -> Router<Arc<HubState>> {
    app.route("/ws/v1/device", get(relay::ws))
        .route("/api/v1/signaling/{*rest}", get(api::not_implemented))
}

pub fn web_routes(app: Router<Arc<HubState>>) -> Router<Arc<HubState>> {
    app.fallback(web::asset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_only_contributed_plugins_in_boot_order() {
        let table = HubRouter::new();
        table.contribute("asterism.hub.web", web_routes);
        table.contribute("asterism.hub.auth", auth_routes);
        table.contribute("asterism.hub.maintenance", maintenance_routes);
        let order = ["asterism.hub.maintenance", "asterism.hub.auth", "asterism.hub.web"];
        let contribs = table.contribs.lock().unwrap().clone();
        let assembled: Vec<_> = order
            .iter()
            .filter(|id| contribs.iter().any(|(plugin, _)| plugin == *id))
            .copied()
            .collect();
        assert_eq!(assembled, order);
    }
}
