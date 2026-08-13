use crate::transport::Route;

/// Phase 2 实现竞速。Phase 1 只保留选择逻辑的纯函数。
pub fn select_route(lan_ready: bool, hub_ready: bool) -> Option<Route> {
    if lan_ready {
        Some(Route::LanDirect)
    } else if hub_ready {
        Some(Route::HubRelay)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_lan() {
        assert_eq!(select_route(true, true), Some(Route::LanDirect));
        assert_eq!(select_route(false, true), Some(Route::HubRelay));
        assert_eq!(select_route(false, false), None);
    }
}
