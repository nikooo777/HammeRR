use std::collections::BTreeSet;
use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ListToolsResult, MetaObject, PaginatedRequestParams,
    ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, ServerHandler, schemars};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{DOCUMENT_META, Workspace, metadata, stamp_result};
use crate::mcp::{EditorServer, close};
use crate::session::Actor;

#[derive(Clone)]
pub struct WorkspaceServer(pub(super) Workspace);

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct EmptyArgs {}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct OpenArgs {
    path: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ShowArgs {
    document_id: String,
    focus: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct QuitArgs {
    /// Exact token from get_quit_review for the complete current workspace.
    review_token: String,
    /// Explicit consent to discard every reviewed unsaved change and draft.
    discard_changes: bool,
}

fn command<T: schemars::JsonSchema>(name: &'static str, description: &'static str) -> Tool {
    let schema =
        serde_json::to_value(schemars::schema_for!(T)).expect("Tool schema serialization failed");
    Tool::new(
        name,
        description,
        schema
            .as_object()
            .expect("Tool schema must be an object")
            .clone(),
    )
}

pub(super) fn tools() -> Result<Vec<Tool>, String> {
    let mut tools = EditorServer::tool_router().list_all();
    for tool in &mut tools {
        let mut schema = Value::Object((*tool.input_schema).clone());
        project_schema(&mut schema).map_err(|error| format!("{}: {error}", tool.name))?;
        tool.input_schema = Arc::new(
            schema
                .as_object()
                .expect("Tool schema must remain an object")
                .clone(),
        );
        if tool.name == close::TOOL_NAME {
            tool.description = Some("Close the pinned document using its fresh get_map_info.document_close review_token and explicit discard_changes consent. With a live peer, drains and retires only this document and returns an ordinary result while the shared MCP service stays open. Closing the last live document verifies exact terminal output before stopping the shared service and quitting. Accepted cleanup survives caller cancellation. For the last visible window with hidden peers, use get_quit_review and quit_all to review and close the complete workspace, or show a hidden peer first.".into());
        }
    }
    tools.extend([
        command::<EmptyArgs>("list_documents", "List up to two fixed documents, opaque routing selectors, cached map/work state, visibility and capacity. Read-only; does not poll game or UI state."),
        command::<EmptyArgs>("get_quit_review", "Inspect the exact complete workspace for Quit all, including hidden documents, identities, unsaved changes, drafts, owned work and active admissions. Returns one combined review_token. Does not close documents. Busy or unavailable workspace documents refuse explicitly."),
        command::<QuitArgs>("quit_all", "Quit every document using the exact fresh get_quit_review review_token and explicit discard_changes consent. Takes no document selector. Preflight validates every document before freezing workspace membership and all Session/admission gates together. Any changed review, busy admitted request, concurrent operation, missing consent or response reservation failure leaves all maps live. Accepted cleanup survives caller cancellation, cancels/drains owned work and retires all UI hosts. Per-document cleanup failures retain failed entries and source claims. Exact terminal response delivery precedes shared MCP service shutdown and native application quit."),
        command::<EmptyArgs>("save_all_named_maps", "Save all named maps through the same Open documents… action. Takes no arguments or document selector. Freezes up to two admitted document identities before scheduling, then attempts each once using its own Session.save and original source claim. Clean named maps verify their original source identity without writing. Unnamed dirty maps require Save as, pending reviews are preserved and block saving, and stale/busy/closing/source/write errors remain per-document results. Partial results return isError with complete:false/status:not_complete. No path invention, file chooser, focus, window show, rollback across documents or automatic retry."),
        command::<EmptyArgs>("new_document", "Create a second independent untitled document without replacing an existing buffer. Desktop creation is hidden until show_document; headless creation stays headless. Rejects full or busy workspace."),
        command::<OpenArgs>("open_document", "Open an absolute VMF path in a new independent document, without replacing existing buffers. Requires a free workspace slot and path claims. Desktop creation is hidden until show_document."),
        command::<ShowArgs>("show_document", "Show the exact desktop document identified by document_id. focus defaults true. focus=false requests silent display through the same Open documents → Show silently operation; requires supported Hyprland Lua rules and refuses before showing if admission fails. Headless workspaces reject; unknown or closing selectors never fall back."),
    ]);
    Ok(tools)
}

fn project_node(
    schema: &mut Value,
    original: &Value,
    references: &mut BTreeSet<String>,
) -> Result<(), String> {
    let reference = schema
        .get("$ref")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(reference) = reference {
        if !references.insert(reference.clone()) {
            return Err("Recursive top-level tool schema reference".into());
        }
        let pointer = reference
            .strip_prefix('#')
            .ok_or("External top-level schema references are unsupported")?;
        let target = original
            .pointer(pointer)
            .ok_or("Top-level schema reference is missing")?
            .clone();
        let mut siblings = schema
            .as_object()
            .ok_or("Tool schema must be an object")?
            .clone();
        siblings.remove("$ref");
        siblings
            .entry("allOf")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or("Tool allOf must be an array")?
            .insert(0, target);
        *schema = Value::Object(siblings);
        project_node(schema, original, references)?;
        references.remove(&reference);
        return Ok(());
    }
    let Some(object) = schema.as_object_mut() else {
        return Err("Top-level tool schema must be an object".into());
    };
    let properties = object
        .entry("properties")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("Tool properties must be an object")?;
    if properties.contains_key("document_id") {
        return Err("document_id is reserved for workspace routing".into());
    }
    properties.insert("document_id".into(), json!({"type":"string","description":"Opaque document selector from list_documents. Omit only when exactly one live document exists."}));
    for key in ["oneOf", "anyOf", "allOf"] {
        if let Some(branches) = object.get_mut(key).and_then(Value::as_array_mut) {
            for branch in branches {
                project_node(branch, original, references)?;
            }
        }
    }
    Ok(())
}

fn project_schema(schema: &mut Value) -> Result<(), String> {
    let original = schema.clone();
    project_node(schema, &original, &mut BTreeSet::new())
}

fn parse<T: serde::de::DeserializeOwned>(
    arguments: Option<rmcp::model::JsonObject>,
) -> Result<T, ErrorData> {
    serde_json::from_value(Value::Object(arguments.unwrap_or_default()))
        .map_err(|error| ErrorData::invalid_params(error.to_string(), None))
}

fn stamp_meta(meta: &mut Option<MetaObject>, context: &Value) {
    meta.get_or_insert_with(MetaObject::new)
        .0
        .insert(DOCUMENT_META.into(), context.clone());
}

fn stamp_response(response: &mut CallToolResponse, context: &Value) {
    match response {
        CallToolResponse::Complete(result) => stamp_result(result, context),
        CallToolResponse::InputRequired(result) => stamp_meta(&mut result.meta, context),
        CallToolResponse::Task(result) => stamp_meta(&mut result.meta, context),
        _ => {}
    }
}

fn stamp_error(mut error: ErrorData, context: &Value) -> ErrorData {
    let mut data = match error.data.take() {
        Some(Value::Object(object)) if !object.contains_key(DOCUMENT_META) => object,
        Some(original) => serde_json::Map::from_iter([("original_data".into(), original)]),
        None => serde_json::Map::new(),
    };
    data.insert(DOCUMENT_META.into(), context.clone());
    error.data = Some(Value::Object(data));
    error
}

impl ServerHandler for WorkspaceServer {
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        tools()
            .map(ListToolsResult::with_all_items)
            .map_err(|error| ErrorData::internal_error(error, None))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        tools().ok()?.into_iter().find(|tool| tool.name == name)
    }

    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(rmcp::model::ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new("hammerr", env!("CARGO_PKG_VERSION")))
            .with_instructions(format!("{}\nWorkspace: use list_documents to obtain opaque document_id selectors. Every document tool accepts this optional top-level string. Omission is allowed only with exactly one live document. Each call stays pinned to that document; focus never changes routing. new_document/open_document add a second independent buffer; show_document explicitly presents it. save_all_named_maps takes no selector and saves the fixed admitted named-map set with explicit per-document failures; unnamed dirty maps require Save as and pending reviews remain unchanged. get_quit_review inspects every map, including hidden peers; quit_all takes that combined review_token and explicit discard_changes consent, then closes every reviewed map and exits after terminal output delivery. Both workspace quit tools take no document selector. Existing new_map/open_map still replace their pinned buffer. Results carry io.hammerr/document metadata without changing content payloads.", crate::mcp::INSTRUCTIONS))
    }

    async fn call_tool(
        &self,
        mut request: CallToolRequestParams,
        context: RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if matches!(
            request.name.as_ref(),
            "list_documents"
                | "save_all_named_maps"
                | "new_document"
                | "open_document"
                | "show_document"
                | "get_quit_review"
                | "quit_all"
        ) {
            let pinned = if request.name == "show_document" {
                request
                    .arguments
                    .as_ref()
                    .and_then(|arguments| arguments.get("document_id"))
                    .and_then(Value::as_str)
                    .and_then(|selector| self.0.0.factory.resolve(Some(selector)).ok())
                    .map(|entry| metadata(&entry))
            } else {
                None
            };
            if context.ct.is_cancelled() {
                let mut result =
                    crate::mcp::fail("The workspace request was cancelled before acceptance")?;
                if let Some(target) = &pinned {
                    stamp_result(&mut result, target);
                }
                return Ok(result.into());
            }
            if request.name == "quit_all" {
                let args: QuitArgs = parse(request.arguments)?;
                let receiver =
                    match self
                        .0
                        .quit_all_mcp(&args.review_token, args.discard_changes, &context)
                    {
                        Ok(receiver) => receiver,
                        Err(error) => return crate::mcp::fail(error).map(Into::into),
                    };
                return match receiver.await {
                    Ok(result) => Ok(result.into()),
                    Err(_) => crate::mcp::fail(
                        "The accepted Quit all result owner was lost; cleanup remains required",
                    )
                    .map(Into::into),
                };
            }
            let value = match request.name.as_ref() {
                "list_documents" => {
                    let _: EmptyArgs = parse(request.arguments)?;
                    self.0.list_documents()
                }
                "get_quit_review" => {
                    let _: EmptyArgs = parse(request.arguments)?;
                    self.0.quit_review().map(|review| review.inspection())
                }
                "save_all_named_maps" => {
                    let _: EmptyArgs = parse(request.arguments)?;
                    self.0.save_all_named_maps(Actor::Agent).await
                }
                "new_document" => {
                    let _: EmptyArgs = parse(request.arguments)?;
                    self.0.new_document(Actor::Agent, false).await
                }
                "open_document" => {
                    let args: OpenArgs = parse(request.arguments)?;
                    self.0
                        .open_document(Actor::Agent, std::path::Path::new(&args.path), false)
                        .await
                }
                "show_document" => {
                    let args: ShowArgs =
                        parse(request.arguments).map_err(|error| match &pinned {
                            Some(target) => stamp_error(error, target),
                            None => error,
                        })?;
                    self.0
                        .show_document_with_focus(&args.document_id, args.focus.unwrap_or(true))
                        .await
                }
                _ => unreachable!(),
            };
            return match value {
                Ok(value) => {
                    let target = value.get("document_id").zip(value.get("runtime_document_id")).map(|(selector, id)| json!({"document_id":selector,"runtime_document_id":id}));
                    let incomplete =
                        request.name == "save_all_named_maps" && value["complete"] != true;
                    let mut result = crate::mcp::ok(value)?;
                    if incomplete {
                        result.is_error = Some(true);
                    }
                    if let Some(target) = target {
                        stamp_result(&mut result, &target);
                    }
                    Ok(result.into())
                }
                Err(error) => {
                    let mut result = crate::mcp::fail(error)?;
                    if let Some(target) = &pinned {
                        stamp_result(&mut result, target);
                    }
                    Ok(result.into())
                }
            };
        }
        let selector = match request
            .arguments
            .as_mut()
            .and_then(|arguments| arguments.remove("document_id"))
        {
            Some(Value::String(selector)) => Some(selector),
            Some(_) => {
                return Err(ErrorData::invalid_params(
                    "document_id must be an opaque string from list_documents",
                    None,
                ));
            }
            None => None,
        };
        let entry = match self.0.0.factory.resolve(selector.as_deref()) {
            Ok(entry) => entry,
            Err(error) => return crate::mcp::fail(error).map(Into::into),
        };
        let target = metadata(&entry);
        let server = self
            .0
            .fixed_server(&entry)
            .map_err(|error| stamp_error(ErrorData::internal_error(error, None), &target))?;
        if request.name.as_ref() == close::TOOL_NAME {
            let args: crate::mcp::params::CloseDocumentArgs =
                parse(request.arguments).map_err(|error| stamp_error(error, &target))?;
            let reservation = match self.0.0.factory.reserve_close(entry.selector()) {
                Ok(reservation) => reservation,
                Err(error) => {
                    let mut result = crate::mcp::fail(error)?;
                    stamp_result(&mut result, &target);
                    return Ok(result.into());
                }
            };
            let terminal = server
                .terminal
                .as_ref()
                .expect("Document route missing Close owner");
            let receiver = match terminal.commit_workspace_mcp(
                &server,
                &args.review_token,
                args.discard_changes,
                &context,
                self.0.close_plan(reservation, target.clone()),
            ) {
                Ok(receiver) => receiver,
                Err(error) => {
                    let mut result = crate::mcp::fail(error)?;
                    stamp_result(&mut result, &target);
                    return Ok(result.into());
                }
            };
            return match receiver.await {
                Ok(result) => Ok(result.into()),
                Err(_) => {
                    let mut result = crate::mcp::fail(
                        "The accepted Close result owner was lost; cleanup remains required",
                    )?;
                    stamp_result(&mut result, &target);
                    Ok(result.into())
                }
            };
        }
        match server.call_tool(request, context).await {
            Ok(mut response) => {
                stamp_response(&mut response, &target);
                Ok(response)
            }
            Err(error) => Err(stamp_error(error, &target)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_projects_all_current_tools_and_preserves_required_fields() {
        let projected = tools().unwrap();
        for original in EditorServer::tool_router().list_all() {
            let tool = projected
                .iter()
                .find(|tool| tool.name == original.name)
                .unwrap();
            assert_eq!(
                tool.input_schema.get("required"),
                original.input_schema.get("required")
            );
            assert_eq!(
                tool.input_schema["properties"]["document_id"]["type"],
                "string"
            );
            for (name, property) in original
                .input_schema
                .get("properties")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                assert_eq!(
                    &tool.input_schema["properties"][name], property,
                    "{} {name}",
                    original.name
                );
            }
        }
        let mut schema = json!({"oneOf":[{"type":"object","properties":{"mode":{"const":"one"}},"required":["mode"],"additionalProperties":false},{"$ref":"#/$defs/Other"}],"$defs":{"Other":{"type":"object","properties":{"mode":{"const":"two"}},"required":["mode"],"additionalProperties":false}}});
        project_schema(&mut schema).unwrap();
        assert_eq!(
            schema["oneOf"][0]["properties"]["document_id"]["type"],
            "string"
        );
        assert_eq!(
            schema["oneOf"][1]["properties"]["document_id"]["type"],
            "string"
        );
        assert!(
            schema["$defs"]["Other"]["properties"]
                .get("document_id")
                .is_none()
        );
        let mut referenced = json!({"$ref":"#/$defs/Args","$defs":{"Args":{"type":"object","properties":{"nested":{"$ref":"#/$defs/Nested"}},"additionalProperties":false},"Nested":{"type":"string"}}});
        let definitions = referenced["$defs"].clone();
        project_schema(&mut referenced).unwrap();
        assert_eq!(referenced["$defs"], definitions);
        assert_eq!(
            referenced["allOf"][0]["properties"]["nested"]["$ref"],
            "#/$defs/Nested"
        );
        assert_eq!(
            referenced["allOf"][0]["properties"]["document_id"]["type"],
            "string"
        );
        let mut collision = json!({"properties":{"document_id":{"type":"integer"}}});
        assert!(project_schema(&mut collision).is_err());
    }

    #[test]
    fn protocol_error_context_preserves_original_data_code_and_message() {
        let target = json!({"document_id":"opaque","runtime_document_id":9});
        for original in [
            json!("source"),
            json!([1, 2]),
            json!({DOCUMENT_META:{"prior":true}}),
            json!({"detail":"source"}),
        ] {
            let source = ErrorData::invalid_params("typed parsing failed", Some(original.clone()));
            let stamped = stamp_error(source.clone(), &target);
            assert_eq!(stamped.code, source.code);
            assert_eq!(stamped.message, source.message);
            assert_eq!(stamped.data.as_ref().unwrap()[DOCUMENT_META], target);
            if original.is_object() && original.get(DOCUMENT_META).is_none() {
                assert_eq!(stamped.data.as_ref().unwrap()["detail"], "source");
            } else {
                assert_eq!(stamped.data.as_ref().unwrap()["original_data"], original);
            }
        }
    }
}
