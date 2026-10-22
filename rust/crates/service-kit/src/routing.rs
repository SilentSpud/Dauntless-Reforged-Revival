//! Express-compatible routing (MIGRATION_PLAN.md M6).
//!
//! The game client and the launcher were written against Express, which matches a path
//! case-insensitively, ignores one trailing slash, answers `HEAD` with the `GET` route, and takes
//! the first registered route that matches. axum's router is case-sensitive and prefers static
//! segments over parameters whatever the order, so [`Router`] matches by itself, in registration
//! order, and hands the request to the route's axum handler. Parameter values keep their case and
//! are percent-decoded; handlers read them with [`Params`]. Patterns are literal segments and
//! `:name` parameters, which is all the Node services use (`routes.json`).
//!
//! Unlike Express, a handler can't pass a request on to the next matching route.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::extract::FromRequestParts;
use axum::handler::Handler;
use axum::http::request::Parts;
use axum::http::{Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodFilter, MethodRouter, on};
use percent_encoding::percent_decode_str;
use tower::{Service, ServiceExt};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Literal(String),
    Param(String),
}

/// A route path such as `/character/:characterId/slot/:index`.
#[derive(Debug, Clone)]
pub struct Pattern {
    path: String,
    segments: Vec<Segment>,
}

/// Why a path that matched a pattern can't be served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadParam(pub String);

impl Pattern {
    /// Panics on a malformed pattern: routes are fixed at build time.
    pub fn parse(path: &str) -> Self {
        assert!(path.starts_with('/'), "route {path:?} must start with /");
        // A trailing slash is Node's spelling of the same route ("/motd/"): matching ignores one
        let segments = split(path.strip_suffix('/').filter(|p| !p.is_empty()).unwrap_or(path))
            .map(|segment| match segment.strip_prefix(':') {
                Some(name) => {
                    assert!(!name.is_empty(), "route {path:?} has an unnamed parameter");
                    Segment::Param(name.to_owned())
                }
                None => {
                    assert!(!segment.is_empty(), "route {path:?} has an empty segment");
                    assert!(
                        !segment.contains(['*', '{', '}', '?', '(', ')']),
                        "route {path:?}: only literals and :params"
                    );
                    Segment::Literal(segment.to_owned())
                }
            })
            .collect();
        Self {
            path: path.to_owned(),
            segments,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.path
    }

    /// `None` when the path doesn't match; the parameters, or a value that isn't valid
    /// percent-encoded UTF-8 (Express answers that with 400), when it does.
    pub fn matches(&self, path: &str) -> Option<Result<Params, BadParam>> {
        let path = match path.strip_suffix('/') {
            Some(rest) if !rest.is_empty() => rest,
            _ => path,
        };
        if !path.starts_with('/') {
            return None;
        }
        let parts: Vec<&str> = split(path).collect();
        if parts.len() != self.segments.len() {
            return None;
        }
        let mut params = Vec::new();
        for (segment, part) in self.segments.iter().zip(&parts) {
            match segment {
                Segment::Literal(literal) if literal.eq_ignore_ascii_case(part) => {}
                Segment::Literal(_) => return None,
                Segment::Param(_) if part.is_empty() => return None,
                Segment::Param(name) => params.push((name.clone(), (*part).to_owned())),
            }
        }
        Some(
            params
                .into_iter()
                .map(|(name, raw)| match percent_decode_str(&raw).decode_utf8() {
                    Ok(value) => Ok((name, value.into_owned())),
                    Err(_) => Err(BadParam(raw)),
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Params),
        )
    }
}

/// The segments after the leading slash; the root path has none.
fn split(path: &str) -> impl Iterator<Item = &str> {
    let rest = &path[1..];
    rest.split('/').filter(move |_| !rest.is_empty())
}

/// The route parameters of a request, by name, decoded and in their original case.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Params(Vec<(String, String)>);

impl Params {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Params {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(parts.extensions.get::<Params>().cloned().unwrap_or_default())
    }
}

/// The route the request matched, for logging: the pattern as registered.
#[derive(Debug, Clone)]
pub struct MatchedPattern(pub Arc<str>);

/// Routes in registration order; the first that matches the method and path serves the request.
pub struct Router<S = ()> {
    routes: Vec<(Method, Pattern, MethodRouter<S>)>,
}

impl<S> Default for Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    pub fn new() -> Self {
        Self { routes: Vec::new() }
    }

    pub fn route<H, T>(mut self, method: Method, path: &str, handler: H) -> Self
    where
        H: Handler<T, S>,
        T: 'static,
    {
        let filter = MethodFilter::try_from(method.clone())
            .unwrap_or_else(|_| panic!("route {path:?}: method {method} can't be routed"));
        self.routes.push((method, Pattern::parse(path), on(filter, handler)));
        self
    }

    pub fn get<H: Handler<T, S>, T: 'static>(self, path: &str, handler: H) -> Self {
        self.route(Method::GET, path, handler)
    }

    pub fn post<H: Handler<T, S>, T: 'static>(self, path: &str, handler: H) -> Self {
        self.route(Method::POST, path, handler)
    }

    pub fn put<H: Handler<T, S>, T: 'static>(self, path: &str, handler: H) -> Self {
        self.route(Method::PUT, path, handler)
    }

    pub fn patch<H: Handler<T, S>, T: 'static>(self, path: &str, handler: H) -> Self {
        self.route(Method::PATCH, path, handler)
    }

    pub fn delete<H: Handler<T, S>, T: 'static>(self, path: &str, handler: H) -> Self {
        self.route(Method::DELETE, path, handler)
    }

    /// Wraps the route added last in `layer`, such as a rate limit or an auth check that runs before
    /// its handler (Express's per-route middleware).
    pub fn layer<L>(mut self, layer: L) -> Self
    where
        L: tower::Layer<axum::routing::Route> + Clone + Send + Sync + 'static,
        L::Service: Service<Request<Body>> + Clone + Send + Sync + 'static,
        <L::Service as Service<Request<Body>>>::Response: IntoResponse + 'static,
        <L::Service as Service<Request<Body>>>::Error: Into<Infallible> + 'static,
        <L::Service as Service<Request<Body>>>::Future: Send + 'static,
    {
        if let Some((method, pattern, route)) = self.routes.pop() {
            self.routes.push((method, pattern, route.layer(layer)));
        }
        self
    }

    /// Appends another router's routes after these, like `app.use("/", router)` in Express.
    pub fn merge(mut self, other: Router<S>) -> Self {
        self.routes.extend(other.routes);
        self
    }

    /// The method and pattern of every route, in match order (checked against `routes.json`).
    pub fn table(&self) -> Vec<(Method, String)> {
        self.routes
            .iter()
            .map(|(method, pattern, _)| (method.clone(), pattern.as_str().to_owned()))
            .collect()
    }

    /// Fixes the state; requests no route matches get `fallback`.
    pub fn into_service(self, state: S, fallback: fn(&Request<Body>) -> Response) -> RouterService {
        let routes = self
            .routes
            .into_iter()
            .map(|(method, pattern, route)| {
                let name: Arc<str> = pattern.as_str().into();
                Route {
                    method,
                    pattern,
                    name,
                    service: route.with_state(state.clone()),
                }
            })
            .collect();
        RouterService {
            routes: Arc::new(routes),
            fallback,
        }
    }
}

struct Route {
    method: Method,
    pattern: Pattern,
    name: Arc<str>,
    service: MethodRouter<()>,
}

/// A [`Router`] with its state: a tower service, mounted with `axum::Router::fallback_service`.
#[derive(Clone)]
pub struct RouterService {
    routes: Arc<Vec<Route>>,
    fallback: fn(&Request<Body>) -> Response,
}

impl Service<Request<Body>> for RouterService {
    type Response = Response;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut request: Request<Body>) -> Self::Future {
        let method = request.method().clone();
        let path = request.uri().path().to_owned();
        for route in self.routes.iter() {
            if route.method != method && !(method == Method::HEAD && route.method == Method::GET) {
                continue;
            }
            match route.pattern.matches(&path) {
                None => continue,
                Some(Err(_)) => return Box::pin(async { Ok(StatusCode::BAD_REQUEST.into_response()) }),
                Some(Ok(params)) => {
                    request.extensions_mut().insert(params);
                    request.extensions_mut().insert(MatchedPattern(route.name.clone()));
                    let service = route.service.clone();
                    return Box::pin(service.oneshot(request));
                }
            }
        }
        let response = (self.fallback)(&request);
        Box::pin(async move { Ok(response) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(pattern: &str, path: &str) -> Option<Vec<(String, String)>> {
        Pattern::parse(pattern)
            .matches(path)
            .map(|r| r.map(|p| p.0).unwrap_or_default())
    }

    #[test]
    fn matches_like_express() {
        assert_eq!(
            params("/account/api/oauth/token", "/Account/API/oauth/TOKEN"),
            Some(vec![])
        );
        assert_eq!(
            params("/account/api/oauth/token", "/account/api/oauth/token/"),
            Some(vec![])
        );
        assert_eq!(params("/account/api/oauth/token", "/account/api/oauth/token//"), None);
        assert_eq!(params("/account/api/oauth/token", "/account/api/oauth"), None);
        assert_eq!(params("/", "/"), Some(vec![]));
        assert_eq!(params("/", "/x"), None);
        assert_eq!(
            params("/character/:characterId/Slot/:index", "/CHARACTER/Ab%20C/slot/3/"),
            Some(vec![
                ("characterId".into(), "Ab C".into()),
                ("index".into(), "3".into())
            ])
        );
        assert_eq!(
            params("/friends/:friendId", "/friends/"),
            None,
            "a parameter is never empty"
        );
    }

    #[test]
    fn a_badly_encoded_parameter_is_an_error() {
        assert_eq!(
            Pattern::parse("/x/:id").matches("/x/%E2%28"),
            Some(Err(BadParam("%E2%28".into())))
        );
    }

    #[test]
    #[should_panic(expected = "only literals and :params")]
    fn wildcards_are_refused() {
        Pattern::parse("/files/*rest");
    }
}
