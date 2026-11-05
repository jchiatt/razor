use super::super::{Error, other};
use super::{Auth, connection_error, elicitation::Handler, resolve};
use crate::protocol::{Connection, Mcp};
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::{
    model::ClientJsonRpcMessage,
    service::{ClientLifecycleMode, ClientServiceExt, RoleClient, RunningService},
    transport::{
        StreamableHttpClientTransport,
        auth::{AuthClient, AuthorizationManager},
        common::client_side_sse::BoxedSseResponse,
        streamable_http_client::{
            StreamableHttpClient, StreamableHttpClientTransportConfig, StreamableHttpError,
            StreamableHttpPostResponse,
        },
    },
};
use std::{collections::HashMap, sync::Arc};

/// Connects with resolved headers; a server without saved credentials gets an empty manager, so
/// one client type serves both and rmcp's transport is built once.
pub(super) async fn connect(
    name: &str,
    config: &Mcp,
    handler: Handler,
    lifecycle: ClientLifecycleMode,
    auth: &Auth,
) -> Result<RunningService<RoleClient, Handler>, Error> {
    let Some(Connection::Http { url, headers, .. }) = &config.connection else {
        return Err(Error::Refused(
            "The MCP server has no URL; configure one.".into(),
        ));
    };
    let mut resolved = HashMap::new();
    for (name, reference) in headers {
        let name = HeaderName::from_bytes(name.as_bytes());
        let value = HeaderValue::from_str(&resolve(reference).await?);
        let (Ok(name), Ok(mut value)) = (name, value) else {
            return Err(Error::Refused(
                "An MCP header is invalid; fix its name or credential reference.".into(),
            ));
        };
        value.set_sensitive(true);
        resolved.insert(name, value);
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| other(e, "Could not create the MCP HTTP client."))?;
    let manager = match auth.manager(name, config).await? {
        Some(manager) => manager,
        None => AuthorizationManager::new(url.as_str()).await.map_err(|_| {
            Error::Refused("The MCP server address is invalid; check its URL.".into())
        })?,
    };
    let mut transport =
        StreamableHttpClientTransportConfig::with_uri(url.clone()).custom_headers(resolved);
    // Session recovery must not replay a call whose effects may already have happened.
    transport.reinit_on_expired_session = false;
    let client = Http(AuthClient::new(client, manager));
    handler
        .serve_with_lifecycle(
            StreamableHttpClientTransport::with_client(client, transport),
            lifecycle,
        )
        .await
        .map_err(connection_error)
}

type Failure = StreamableHttpError<reqwest::Error>;

/// Each request runs on its own task, so a dropped waiter cannot cancel a rotating-token
/// exchange before its save. rmcp's size-bounded variants default to these methods.
#[derive(Clone)]
struct Http(AuthClient<reqwest::Client>);

async fn owned<T: Send + 'static>(
    future: impl Future<Output = Result<T, Failure>> + Send + 'static,
) -> Result<T, Failure> {
    tokio::spawn(future)
        .await
        .map_err(StreamableHttpError::TokioJoinError)?
}

impl StreamableHttpClient for Http {
    type Error = reqwest::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session: Option<Arc<str>>,
        auth: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, Failure> {
        let client = self.0.clone();
        owned(async move {
            client
                .post_message(uri, message, session, auth, headers)
                .await
        })
        .await
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session: Option<Arc<str>>,
        cursor: Option<String>,
        auth: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxedSseResponse, Failure> {
        let client = self.0.clone();
        owned(async move { client.get_stream(uri, session, cursor, auth, headers).await }).await
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session: Arc<str>,
        auth: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), Failure> {
        let client = self.0.clone();
        owned(async move { client.delete_session(uri, session, auth, headers).await }).await
    }
}
