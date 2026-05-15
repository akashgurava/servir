use axum::Router;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer},
};
use tracing::Level;

/// Applies the standard middleware stack to a router.
///
/// Layer order (outermost → innermost, i.e. first to see the request):
///   1. `SetRequestIdLayer`      — assigns `x-request-id` if absent (UUID v4)
///   2. `PropagateRequestIdLayer`— echoes `x-request-id` back on the response
///   3. `TraceLayer`             — emits an INFO span per request (method, uri, status, latency)
///
/// Accepts `Router<()>` — call `.with_state(state)` before passing in.
/// Additional layers can be composed by the caller before or after this call.
pub fn standard_middleware(router: Router) -> Router {
    router
        // TraceLayer is innermost: request ID is already set when the span opens.
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(PropagateRequestIdLayer::x_request_id())
        // SetRequestIdLayer is outermost: runs first on every inbound request.
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
}
