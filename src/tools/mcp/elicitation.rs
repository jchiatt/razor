use super::super::Error;
use crate::profiles::mcp_text;
use futures_util::future::BoxFuture;
use rmcp::{
    ClientHandler,
    model::{
        ClientConfig, ElicitRequestParams, ElicitResult, ElicitationAction, ElicitationCapability,
        ErrorData, FormElicitationCapability, UrlElicitationCapability,
    },
    service::{NotificationContext, RequestContext, RoleClient},
};
use std::sync::Arc;
use tokio::sync::Notify;

type Ask = dyn Fn(String, Vec<String>) -> BoxFuture<'static, Result<String, Error>> + Send + Sync;

/// Routes server questions through the person's existing question path, outside action locks.
#[derive(Clone)]
pub(crate) struct Questions(pub(crate) Arc<Ask>);

#[derive(Clone)]
pub(super) struct Handler {
    pub(super) name: String,
    pub(super) changed: Arc<Notify>,
    pub(super) questions: Questions,
}

impl ClientHandler for Handler {
    fn get_info(&self) -> ClientConfig {
        let mut info = ClientConfig::default();
        info.capabilities.elicitation = Some(
            ElicitationCapability::default()
                .with_form(FormElicitationCapability::default())
                .with_url(UrlElicitationCapability::default()),
        );
        info
    }

    async fn on_tool_list_changed(&self, _: NotificationContext<RoleClient>) {
        self.changed.notify_one();
    }

    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        elicit(&self.name, &self.questions, request)
            .await
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))
    }
}

/// One server question to the person. A form's answer must be a JSON object; the specification
/// leaves checking it against the form's flat schema to the server, which validates its own.
pub(super) async fn elicit(
    name: &str,
    questions: &Questions,
    request: ElicitRequestParams,
) -> Result<ElicitResult, Error> {
    let [accept, decline, cancel] = ["accept", "decline", "cancel"].map(|k| mcp_text(k, &[]));
    let (question, form) = match request {
        ElicitRequestParams::FormElicitationParams {
            message,
            requested_schema,
            ..
        } => {
            let fields = serde_json::to_string(&requested_schema).unwrap_or_default();
            let fields = [("name", name), ("message", &message), ("fields", &fields)];
            (mcp_text("form", &fields), true)
        }
        ElicitRequestParams::UrlElicitationParams { message, url, .. } => {
            let host = reqwest::Url::parse(&url)
                .ok()
                .filter(|u| matches!(u.scheme(), "https" | "http"))
                .and_then(|u| u.host_str().map(str::to_owned))
                .ok_or_else(|| Error::Refused(mcp_text("elicitation_invalid_url", &[])))?;
            let fields = [
                ("name", name),
                ("host", &host),
                ("message", &message),
                ("url", &url),
            ];
            (mcp_text("url", &fields), false)
        }
        _ => return Err(Error::Refused(mcp_text("elicitation_unsupported", &[]))),
    };
    let mut choices = vec![decline.clone(), cancel.clone()];
    if !form {
        choices.insert(0, accept.clone());
    }
    let answer = (questions.0)(question, choices).await?;
    let action = match answer {
        a if a == decline => ElicitationAction::Decline,
        a if a == cancel => ElicitationAction::Cancel,
        a if !form && a == accept => ElicitationAction::Accept,
        a => {
            let value = serde_json::from_str::<serde_json::Value>(&a).ok();
            let Some(object) = value.filter(|v| form && v.is_object()) else {
                return Err(Error::Refused(mcp_text("invalid_form", &[])));
            };
            let mut result = ElicitResult::new(ElicitationAction::Accept);
            result.content = Some(object);
            return Ok(result);
        }
    };
    Ok(ElicitResult::new(action))
}
