use super::*;
use my_ai_agent::my_json::{
    self,
    json_writer::{JsonObjectWriter, RawJsonObject},
};

pub fn compile_init_response(
    name: &str,
    version: &str,
    instructions: &str,
    protocol_version: &str,
    id: i64,
    has_tools: bool,
    has_resources: bool,
    has_prompts: bool,
) -> String {
    let json_builder =
        my_json::json_writer::JsonObjectWriter::new().write_json_object("result", |result| {
            result
                .write("protocolVersion", protocol_version)
                .write_json_object("capabilities", |cap| {
                    cap.write_json_object_if("resources", has_resources, |res| {
                        res.write("listChanged", true)
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

pub fn compile_tool_calls(tools: Vec<ToolCallSchemaData>, id: i64) -> String {
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

pub fn compile_prompts_list(prompts: Vec<super::PromptSchemaData>, id: i64) -> String {
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

pub fn compile_get_prompt_response(response: PromptExecutionResult, id: i64) -> String {
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
    id: i64,
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

pub fn compile_read_resource_response(response: ResourceReadResult, id: i64) -> String {
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

pub fn compile_execute_tool_call_response(response: String, id: i64, is_error: bool) -> String {
    let mut result = JsonObjectWriter::new()
        .write("jsonrpc", "2.0")
        .write("id", id)
        .write_json_object("result", |result| {
            result
                .write_json_array("content", |arr| {
                    arr.write_json_object(|obj| {
                        obj.write("type", "text").write("text", response.as_str())
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

pub fn build_ping_response(id: i64) -> String {
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

pub fn build(json: JsonObjectWriter, id: i64) -> String {
    let mut result = "data: ".to_string();
    json.write("jsonrpc", "2.0")
        .write("id", id)
        .build_into(&mut result);

    result.push('\n');
    result.push('\n');
    result
}
