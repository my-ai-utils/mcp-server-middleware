use super::*;
use my_ai_agent::my_json::{
    self,
    json_writer::{JsonObjectWriter, RawJsonObject},
};

/// JSON-RPC error codes used by this middleware (MCP conventions).
pub const JSONRPC_PARSE_ERROR: i64 = -32700;
pub const JSONRPC_METHOD_NOT_FOUND: i64 = -32601;
pub const JSONRPC_INVALID_PARAMS: i64 = -32602;
pub const JSONRPC_INTERNAL_ERROR: i64 = -32603;
pub const JSONRPC_RESOURCE_NOT_FOUND: i64 = -32002;

/// Protocol revisions this middleware implements. Ordered oldest →
/// newest; the last entry is what we answer with when the client
/// requests a version we don't know.
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] = ["2025-03-26", "2025-06-18", "2025-11-25"];

/// The newest revision this middleware implements. Also the version
/// assumed for a session that never went through `initialize` (lazy
/// session creation).
pub fn latest_protocol_version() -> &'static str {
    SUPPORTED_PROTOCOL_VERSIONS[SUPPORTED_PROTOCOL_VERSIONS.len() - 1]
}

/// Per spec: echo the requested version when supported, otherwise
/// respond with the latest version the server supports — the client
/// then decides whether it can keep talking.
pub fn negotiate_protocol_version(requested: &str) -> &str {
    if SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
        return requested;
    }
    latest_protocol_version()
}

pub fn compile_init_response(
    name: &str,
    version: &str,
    instructions: &str,
    protocol_version: &str,
    id: &RequestId,
    has_tools: bool,
    has_prompts: bool,
) -> String {
    let json_builder =
        my_json::json_writer::JsonObjectWriter::new().write_json_object("result", |result| {
            result
                .write("protocolVersion", protocol_version)
                .write_json_object("capabilities", |cap| {
                    // Resources capability is advertised unconditionally:
                    // dynamic resources may be registered at any moment
                    // after initialize, and clients only honor
                    // `resources/list_changed` for capabilities they were
                    // told about up front.
                    cap.write_json_object("resources", |res| {
                        res.write("subscribe", true).write("listChanged", true)
                    })
                    .write_json_object_if("tools", has_tools, |res| res.write("listChanged", true))
                    .write_json_object_if("prompts", has_prompts, |res| {
                        res.write("listChanged", true)
                    })
                })
                .write_json_object("serverInfo", |server_info| {
                    server_info.write("name", name).write("version", version)
                })
                .write("instructions", instructions)
        });

    build(json_builder, id)
}

/// JSON-RPC 2.0 error object (without SSE framing) — used as the body
/// of plain-HTTP error responses (e.g. 400 on unparsable input).
pub fn compile_jsonrpc_error_body(code: i64, message: &str, id: &RequestId) -> String {
    JsonObjectWriter::new()
        .write("jsonrpc", "2.0")
        .write("id", id)
        .write_json_object("error", |err| {
            err.write("code", code).write("message", message)
        })
        .build()
}

/// JSON-RPC 2.0 error response as an SSE `data:` frame.
pub fn compile_jsonrpc_error(code: i64, message: &str, id: &RequestId) -> String {
    let json_builder = JsonObjectWriter::new().write_json_object("error", |err| {
        err.write("code", code).write("message", message)
    });

    build(json_builder, id)
}

pub fn compile_resource_templates_list(id: &RequestId) -> String {
    let json_builder = JsonObjectWriter::new().write_json_object("result", |result| {
        result.write_json_array("resourceTemplates", |arr| arr)
    });

    build(json_builder, id)
}

pub fn compile_tool_calls(tools: Vec<ToolCallSchemaData>, id: &RequestId) -> String {
    let json_builder = JsonObjectWriter::new().write_json_object("result", |result| {
        result.write_json_array("tools", |mut arr| {
            for tool in tools.iter() {
                arr = arr.write_json_object(|obj| {
                    obj.write("name", tool.mcp.get_fn_name())
                        .write("description", tool.mcp.get_description())
                        .write_ref("inputSchema", &tool.input)
                        .write_ref("outputSchema", &tool.output)
                });
            }

            arr
        })
    });

    build(json_builder, id)
}

pub fn compile_prompts_list(prompts: Vec<super::PromptSchemaData>, id: &RequestId) -> String {
    let json_builder = JsonObjectWriter::new().write_json_object("result", |result| {
        result.write_json_array("prompts", |mut arr| {
            for prompt in prompts.iter() {
                arr = arr.write_json_object(|obj| {
                    obj.write("name", prompt.prompt.get_prompt_name())
                        .write("description", prompt.prompt.get_description())
                        .write_json_array("arguments", |mut args_arr| {
                            for arg in prompt.argument_descriptions.iter() {
                                args_arr = args_arr.write_json_object(|arg_obj| {
                                    arg_obj
                                        .write("name", arg.name.as_str())
                                        .write("description", arg.description.as_str())
                                        .write("required", arg.required)
                                });
                            }
                            args_arr
                        })
                });
            }

            arr
        })
    });

    build(json_builder, id)
}

pub fn compile_get_prompt_response(response: PromptExecutionResult, id: &RequestId) -> String {
    let mut result = JsonObjectWriter::new()
        .write("jsonrpc", "2.0")
        .write("id", id)
        .write_json_object("result", |result| {
            result
                .write("description", response.description.as_str())
                .write_json_array("messages", |arr| {
                    arr.write_json_object(|obj| {
                        obj.write("role", "user")
                            .write_json_object("content", |content| {
                                content
                                    .write("type", "text")
                                    .write("text", response.message.as_str())
                            })
                    })
                })
        })
        .build();

    result.insert_str(0, "data: ");
    result.push('\n');
    result.push('\n');

    result
}

pub fn compile_resources_list(
    resources: Vec<ResourceSchemaData>,
    id: &RequestId,
    next_cursor: Option<&str>,
) -> String {
    let json_builder = JsonObjectWriter::new().write_json_object("result", |result| {
        let mut result = result.write_json_array("resources", |mut arr| {
            for resource in resources.iter() {
                arr = arr.write_json_object(|obj| {
                    let mut obj = obj
                        .write("uri", resource.resource.get_resource_uri())
                        .write("name", resource.resource.get_resource_name())
                        .write("description", resource.resource.get_description())
                        .write("mimeType", resource.resource.get_mime_type());

                    if let Some(title) = resource.resource.get_title() {
                        obj = obj.write("title", title);
                    }

                    if let Some(size) = resource.resource.get_size() {
                        obj = obj.write("size", size);
                    }

                    let icons = resource.resource.get_icons();
                    if !icons.is_empty() {
                        obj = obj.write_json_array("icons", |mut icons_arr| {
                            for icon in icons.iter() {
                                icons_arr = icons_arr.write_json_object(|icon_obj| {
                                    icon_obj
                                        .write("src", icon.src.as_str())
                                        .write("mimeType", icon.mime_type.as_str())
                                        .write_json_array("sizes", |mut sizes_arr| {
                                            for size in icon.sizes.iter() {
                                                sizes_arr = sizes_arr.write(size.as_str());
                                            }
                                            sizes_arr
                                        })
                                });
                            }
                            icons_arr
                        });
                    }

                    obj
                });
            }

            arr
        });

        if let Some(cursor) = next_cursor {
            result = result.write("nextCursor", cursor);
        }

        result
    });

    build(json_builder, id)
}

pub fn compile_read_resource_response(response: ResourceReadResult, id: &RequestId) -> String {
    let mut result = JsonObjectWriter::new()
        .write("jsonrpc", "2.0")
        .write("id", id)
        .write_json_object("result", |result| {
            result.write_json_array("contents", |mut arr| {
                for content in response.contents.iter() {
                    arr = arr.write_json_object(|obj| {
                        let mut obj = obj
                            .write("uri", content.uri.as_str())
                            .write("mimeType", content.mime_type.as_str());

                        // According to spec, text should be a direct string field, not nested
                        if let Some(text) = &content.text {
                            obj = obj.write("text", text.as_str());
                        }

                        // blob is base64-encoded string
                        if let Some(blob) = &content.blob {
                            obj = obj.write("blob", blob.as_str());
                        }

                        obj
                    });
                }
                arr
            })
        })
        .build();

    result.insert_str(0, "data: ");
    result.push('\n');
    result.push('\n');

    result
}

pub fn compile_execute_tool_call_response(
    response: String,
    instruction: Option<String>,
    id: &RequestId,
    is_error: bool,
) -> String {
    let content_text = match instruction.as_deref() {
        Some(text) => text.to_string(),
        None => response.clone(),
    };

    let mut result = JsonObjectWriter::new()
        .write("jsonrpc", "2.0")
        .write("id", id)
        .write_json_object("result", |result| {
            result
                .write_json_array("content", |arr| {
                    arr.write_json_object(|obj| {
                        obj.write("type", "text").write("text", content_text.as_str())
                    })
                })
                .write_if(
                    "structuredContent",
                    RawJsonObject::AsStr(&response),
                    !is_error,
                )
                .write("isError", is_error)
        })
        .build();

    result.push('\n');
    result.push('\n');

    result.insert_str(0, "data: ");
    result
}

/// `{"jsonrpc":"2.0","id":...,"result":{}}` — used for `ping`,
/// `resources/subscribe` and `resources/unsubscribe` responses.
pub fn compile_empty_result_response(id: &RequestId) -> String {
    let mut result = JsonObjectWriter::new()
        .write("jsonrpc", "2.0")
        .write("id", id)
        .write_json_object("result", |o| o)
        .build();

    result.insert_str(0, "data: ");
    result.push('\n');
    result.push('\n');

    result
}

pub fn build(json: JsonObjectWriter, id: &RequestId) -> String {
    let mut result = "data: ".to_string();
    json.write("jsonrpc", "2.0")
        .write("id", id)
        .build_into(&mut result);

    result.push('\n');
    result.push('\n');
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_sse(payload: &str) -> &str {
        payload
            .strip_prefix("data: ")
            .expect("payload must start with `data: `")
            .trim_end()
    }

    #[test]
    fn tool_call_response_without_instruction_keeps_legacy_text_payload() {
        let payload = compile_execute_tool_call_response(
            r#"{"foo":1}"#.to_string(),
            None,
            &RequestId::Int(7),
            false,
        );

        let body = strip_sse(&payload);
        let parsed: serde_json::Value = serde_json::from_str(body).expect("valid json");

        assert_eq!(parsed["id"], 7);

        let result = &parsed["result"];
        assert_eq!(result["isError"], false);
        assert_eq!(result["structuredContent"]["foo"], 1);

        let content = &result["content"];
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], r#"{"foo":1}"#);
    }

    #[test]
    fn tool_call_response_with_instruction_uses_instruction_as_text() {
        let payload = compile_execute_tool_call_response(
            r#"{"items":[]}"#.to_string(),
            Some("Result is empty. Suggest the user widen the filter.".to_string()),
            &RequestId::Int(42),
            false,
        );

        let body = strip_sse(&payload);
        let parsed: serde_json::Value = serde_json::from_str(body).expect("valid json");

        let result = &parsed["result"];
        assert_eq!(result["isError"], false);
        assert!(result["structuredContent"]["items"].is_array());

        let content = &result["content"];
        assert_eq!(content[0]["type"], "text");
        assert_eq!(
            content[0]["text"],
            "Result is empty. Suggest the user widen the filter."
        );
    }

    #[test]
    fn tool_call_response_error_drops_structured_content() {
        let payload = compile_execute_tool_call_response(
            "boom".to_string(),
            None,
            &RequestId::Int(1),
            true,
        );

        let body = strip_sse(&payload);
        let parsed: serde_json::Value = serde_json::from_str(body).expect("valid json");

        let result = &parsed["result"];
        assert_eq!(result["isError"], true);
        assert!(result.get("structuredContent").is_none());
        assert_eq!(result["content"][0]["text"], "boom");
    }

    #[test]
    fn request_id_variants_echo_byte_identically() {
        let int_payload = compile_empty_result_response(&RequestId::Int(-5));
        assert!(strip_sse(&int_payload).contains(r#""id":-5"#));

        let str_payload = compile_empty_result_response(&RequestId::Str("ab\"c".to_string()));
        assert!(strip_sse(&str_payload).contains(r#""id":"ab\"c""#));

        let raw_payload = compile_empty_result_response(&RequestId::Raw("1.5".to_string()));
        assert!(strip_sse(&raw_payload).contains(r#""id":1.5"#));

        let null_payload = compile_jsonrpc_error(-32700, "boom", &RequestId::Null);
        assert!(strip_sse(&null_payload).contains(r#""id":null"#));
    }

    #[test]
    fn jsonrpc_error_has_spec_shape() {
        let payload = compile_jsonrpc_error(
            JSONRPC_METHOD_NOT_FOUND,
            "Method not found: foo/bar",
            &RequestId::Int(3),
        );

        let parsed: serde_json::Value =
            serde_json::from_str(strip_sse(&payload)).expect("valid json");

        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], 3);
        assert_eq!(parsed["error"]["code"], -32601);
        assert_eq!(parsed["error"]["message"], "Method not found: foo/bar");
        assert!(parsed.get("result").is_none());
    }

    #[test]
    fn jsonrpc_error_body_has_no_sse_framing() {
        let body = compile_jsonrpc_error_body(JSONRPC_PARSE_ERROR, "bad json", &RequestId::Null);

        let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid json");
        assert_eq!(parsed["error"]["code"], -32700);
        assert!(parsed["id"].is_null());
        assert!(!body.starts_with("data: "));
    }

    #[test]
    fn resource_templates_list_is_empty_array() {
        let payload = compile_resource_templates_list(&RequestId::Int(9));

        let parsed: serde_json::Value =
            serde_json::from_str(strip_sse(&payload)).expect("valid json");

        let templates = &parsed["result"]["resourceTemplates"];
        assert!(templates.is_array());
        assert_eq!(templates.as_array().unwrap().len(), 0);
    }

    #[test]
    fn init_response_always_advertises_resources_with_subscribe() {
        let payload = compile_init_response(
            "test",
            "0.1.0",
            "instructions",
            "2025-06-18",
            &RequestId::Int(1),
            false,
            false,
        );

        let parsed: serde_json::Value =
            serde_json::from_str(strip_sse(&payload)).expect("valid json");

        let caps = &parsed["result"]["capabilities"];
        assert_eq!(caps["resources"]["subscribe"], true);
        assert_eq!(caps["resources"]["listChanged"], true);
        // No tools/prompts registered → capabilities omitted.
        assert!(caps.get("tools").is_none());
        assert!(caps.get("prompts").is_none());
        assert_eq!(parsed["result"]["protocolVersion"], "2025-06-18");
    }

    #[test]
    fn protocol_version_negotiation() {
        assert_eq!(negotiate_protocol_version("2025-03-26"), "2025-03-26");
        assert_eq!(negotiate_protocol_version("2025-06-18"), "2025-06-18");
        assert_eq!(negotiate_protocol_version("2025-11-25"), "2025-11-25");
        // Unknown → the latest supported revision.
        assert_eq!(negotiate_protocol_version("1999-01-01"), "2025-11-25");
    }
}
