//! This module defines the [`AppRoutes`] struct that is responsible for
//! configuring routes in an Axum application. It allows you to define route
//! prefixes, add routes, and configure middlewares for the application.

use axum::{http::Request, Router as AXRouter};
use lazy_static::lazy_static;
use regex::Regex;
use tower_http::{catch_panic::CatchPanicLayer, trace::TraceLayer};

use super::routes::Routes;
use crate::{app::AppContext, config, Result};

lazy_static! {
    static ref NORMALIZE_URL: Regex = Regex::new(r"/+").unwrap();
}

/// Represents the routes of the application.
#[derive(Clone)]
pub struct AppRoutes {
    prefix: Option<String>,
    routes: Vec<Routes>,
}

pub struct ListRoutes {
    pub uri: String,
    pub actions: Vec<axum::http::Method>,
    pub method: axum::routing::MethodRouter<AppContext>,
}

impl ToString for ListRoutes {
    fn to_string(&self) -> String {
        let actions_str = self
            .actions
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        // Define your custom logic here to format the struct as a string
        format!("[{}] {}", actions_str, self.uri)
    }
}

impl AppRoutes {
    /// Create a new instance with the default routes.
    #[must_use]
    pub fn with_default_routes() -> Self {
        let routes = Self::empty().add_route(super::ping::routes());
        #[cfg(feature = "with-db")]
        let routes = routes.add_route(super::health::routes());

        routes
    }

    /// Create an empty instance.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            prefix: None,
            routes: vec![],
        }
    }

    #[must_use]
    pub fn collect(&self) -> Vec<ListRoutes> {
        let base_url_prefix = self.get_prefix().map_or("/", |url| url.as_str());

        self.get_routes()
            .iter()
            .flat_map(|router| {
                let mut uri_parts = vec![base_url_prefix];
                if let Some(prefix) = router.prefix.as_ref() {
                    uri_parts.push(prefix);
                }
                router.handlers.iter().map(move |controller| {
                    let uri = format!("{}{}", uri_parts.join("/"), &controller.uri);
                    let binding = NORMALIZE_URL.replace_all(&uri, "/");

                    let uri = if binding.len() > 1 {
                        NORMALIZE_URL
                            .replace_all(&uri, "/")
                            .strip_suffix('/')
                            .map_or_else(|| binding.to_string(), std::string::ToString::to_string)
                    } else {
                        binding.to_string()
                    };

                    ListRoutes {
                        uri,
                        actions: controller.actions.clone(),
                        method: controller.method.clone(),
                    }
                })
            })
            .collect()
    }

    /// Get the prefix of the routes.
    #[must_use]
    pub fn get_prefix(&self) -> Option<&String> {
        self.prefix.as_ref()
    }

    /// Get the routes.
    #[must_use]
    pub fn get_routes(&self) -> &[Routes] {
        self.routes.as_ref()
    }

    /// Set a prefix for the routes. this prefix will be a prefix for all the
    /// routes.
    ///
    /// # Example
    ///
    /// In the following example you are adding api as a prefix for all routes
    ///
    /// ```rust
    /// use loco_rs::controller::AppRoutes;
    ///
    /// AppRoutes::with_default_routes().prefix("api");
    /// ```
    #[must_use]
    pub fn prefix(mut self, prefix: &str) -> Self {
        self.prefix = Some(prefix.to_string());
        self
    }

    /// Add a single route.
    #[must_use]
    pub fn add_route(mut self, route: Routes) -> Self {
        self.routes.push(route);
        self
    }

    /// Add multiple routes.
    #[must_use]
    pub fn add_routes(mut self, mounts: Vec<Routes>) -> Self {
        for mount in mounts {
            self.routes.push(mount);
        }
        self
    }

    /// Convert the routes to an Axum Router, and set a list of middlewares that
    /// configure in the [`config::Config`]
    ///
    /// # Errors
    /// Return an [`Result`] when could not convert the router setup to
    /// [`axum::Router`].
    #[allow(clippy::cognitive_complexity)]
    pub fn to_router(&self, ctx: AppContext) -> Result<AXRouter> {
        let mut app = AXRouter::new();

        for router in self.collect() {
            tracing::info!("{}", router.to_string());

            app = app.route(&router.uri, router.method);
        }

        if let Some(catch_panic) = &ctx.config.server.middlewares.catch_panic {
            if catch_panic.enable {
                app = Self::add_catch_panic(app);
            }
        }

        if let Some(limit) = &ctx.config.server.middlewares.limit_payload {
            if limit.enable {
                app = Self::add_limit_payload_middleware(app, limit)?;
            }
        }

        if let Some(logger) = &ctx.config.server.middlewares.logger {
            if logger.enable {
                app = Self::add_logger_middleware(app);
            }
        }
        Ok(app.with_state(ctx))
    }

    fn add_catch_panic(app: AXRouter<AppContext>) -> AXRouter<AppContext> {
        app.layer(CatchPanicLayer::new())
    }
    fn add_limit_payload_middleware(
        app: AXRouter<AppContext>,
        limit: &config::LimitPayloadMiddleware,
    ) -> Result<AXRouter<AppContext>> {
        let app = app.layer(axum::extract::DefaultBodyLimit::max(
            byte_unit::Byte::from_str(&limit.body_limit)
                .map_err(Box::from)?
                .get_bytes() as usize,
        ));
        tracing::info!(
            data = &limit.body_limit,
            "[Middleware] Adding limit payload",
        );

        Ok(app)
    }
    fn add_logger_middleware(app: AXRouter<AppContext>) -> AXRouter<AppContext> {
        let app = app.layer(
            TraceLayer::new_for_http().make_span_with(|request: &Request<_>| {
                let request_id = uuid::Uuid::new_v4();
                tracing::error_span!(
                    "request",
                    method = tracing::field::display(request.method()),
                    uri = tracing::field::display(request.uri()),
                    version = tracing::field::debug(request.version()),
                    request_id = tracing::field::display(request_id),
                )
            }),
        );

        tracing::info!("[Middleware] Adding log trace id",);
        app
    }
}
