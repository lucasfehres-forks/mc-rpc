use serde_json::Value;
use std::{env, fs::write};

fn main() {
    let json_schema = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/schema.json"
    )))
    .expect("Failed to deserialize RPC Schema");

    let code = generate(&json_schema).expect("Failed to generate json rpc bindings");

    write(
        format!("{}/json_rpc_bindings.rs", env::var("OUT_DIR").unwrap()),
        code,
    )
    .expect("Failed to write json_rpc_bindings.rs");
}

const CURLY: [char; 2] = ['{', '}'];
const IDENTATION: &'static str = "    ";
const FN_IDENTATION: &'static str = "        ";
const DEFAULT_DERIVES: &'static str =
    "#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash)]";

pub fn generate(schema: &Value) -> Option<String> {
    let mut code = String::new();

    code.push_str(dependencies());
    code.push('\n');

    code.push_str(&base_client());
    code.push('\n');

    println!("schemas");
    for (parent_key, schema) in schema.get("components")?.get("schemas")?.as_object()? {
        // its either a struct or an enum so we just check both
        let schema_code = if let Some(c) = EnumData::from_value(&parent_key, &schema) {
            c.to_code()
        } else if let Some(c) = StructData::from_value(&parent_key, &schema) {
            c.to_code()
        } else {
            return None;
        };

        code.push_str(&schema_code);
    }

    // wrap all methods inside the base client
    code.push_str(&format!("impl Client {}\n", CURLY[0]));
    for method in schema.get("methods")?.as_array()? {
        code.push_str(&FunctionData::from_value(&method)?.to_code());
    }
    code.push_str(&format!("\n{}", CURLY[1]));

    Some(code)
}

fn dependencies() -> &'static str {
    r#"
use std::{result::Result as StdResult, time::Duration};
use serde::{Deserialize, Serialize};
use futures_util::TryStreamExt as _;
use tokio_stream::{Stream, wrappers::{BroadcastStream, errors::BroadcastStreamRecvError}};
pub use pale::{ClientConfig, Result, PaleError, RPCError, StreamExt, WebSocketConfig};"#
}

fn base_client() -> String {
    // this is just because it messes up my code highlighter when using curlies in weird context in string literals
    format!(
        r#"
#[derive(Debug, Clone)]
pub struct Client(pub(crate) pale::Client);

impl Client {0}
    pub async fn new(uri: impl AsRef<str>, config: ClientConfig) -> Result<Self> {0}
        Ok(Self(pale::Client::new(uri, config).await?))
    {1}

    pub async fn from_client(client: pale::Client) -> Self {0}
        Ok(Self(client))
    {1}

    /// Calling [`Self::close`] means:
    /// - Closing the underlying connection.
    /// - Any and all internal client communication
    /// - Closing every subscription stream
    ///
    /// The [`Client`] is not guaranteed to be 100% closed after this function returns.
    /// It may take a little while, use [`Self::wait_for_connection`] to make sure before, let's say reconnecting.
    pub async fn close(&self) -> Result<()> {0}
        self.0.close().await
    {1}

    /// Returns if the underlying socket is actively connected.
    pub async fn is_connected(&self) -> bool {0}
        self.0.is_connected().await
    {1}

    /// Returns when the [`Self::is_connected`] is equal to `state`
    ///
    /// If [`Self::is_connected`] already matches `state`, it instantly returns
    ///
    /// ## Example
    /// ```no_run
    /// // waits for the underlying connection to be ready & connected
    /// client.wait_for_connection(true, Duration::from_secs(5)).await;
    ///
    /// // waits for the connection to disconnect
    /// client.wait_for_connection(false, Duration::from_secs(5)).await;
    /// ```
    pub async fn wait_for_connection(&self, state: bool, timeout_duration: Duration) -> Result<()> {0}
        self.0.wait_for_connection(state, timeout_duration).await
    {1}

    /// Returns a [`Stream`] where a message of type [`Client`] will be sent upon each successful reconnection.
    pub fn on_reconnect(&self) -> impl Stream<Item = StdResult<Self, BroadcastStreamRecvError>> {0}
        BroadcastStream::new(self.0.on_reconnect()).map_ok(Self)
    {1}

    /// Returns a [`Stream`] where a message of type [`Client`] will be sent upon disconnect.
    pub fn on_disconnect(&self) -> impl Stream<Item = StdResult<Self, BroadcastStreamRecvError>> {0}
        BroadcastStream::new(self.0.on_disconnect()).map_ok(Self)
    {1}

{1}"#,
        CURLY[0], CURLY[1]
    )
}

#[derive(Debug, Clone)]
struct RustType(String);
impl RustType {
    fn new(type_data: &Value, struct_key: Option<&str>, parent_key: Option<&str>) -> Option<Self> {
        let _type = if type_data.get("enum").is_some()
            || type_data.get("type").unwrap_or(&Value::Null).is_array()
        {
            // the enum/union type for this will get checked from the callers code generation
            match (struct_key, parent_key) {
                (Some(struct_key), Some(parent_key)) => {
                    schema_type_to_rust(&format!("{struct_key}_{parent_key}"))
                }
                _ => return None,
            }
        } else if let Some(_type) = type_data.get("type") {
            let _type = _type.as_str()?;
            if _type == "array" {
                let items = type_data.get("items")?.as_object()?;
                let vec_type = if let Some(item_type) = items.get("type") {
                    item_type.as_str()?
                } else if let Some(item_ref) = items.get("$ref") {
                    item_ref.as_str()?.split('/').last()?
                } else {
                    return None;
                };
                format!("Vec<{}>", schema_type_to_rust(vec_type))
            } else {
                schema_type_to_rust(_type)
            }
        } else if let Some(_ref) = type_data.get("$ref") {
            schema_type_to_rust(_ref.as_str()?.split('/').last()?)
        } else {
            return None;
        };

        Some(Self(_type))
    }

    fn new_empty() -> Self {
        Self("()".to_string())
    }

    fn inner(&self) -> &str {
        &self.0
    }
}

/// Returns a bool that indicates if the string was modified & as well as making the text snake_case
fn field_case(text: &str) -> (String, bool) {
    let chars = text.chars().collect::<Vec<char>>();
    let mut new_name = String::new();
    let mut is_renamed = false;
    for char in chars {
        if char.is_ascii_uppercase() {
            new_name.push('_');
            is_renamed = true;
        }

        new_name.push(char.to_ascii_lowercase());
    }

    // rust grr
    if new_name == "type" {
        new_name = "_type".to_string();
        is_renamed = true;
    }

    (new_name, is_renamed)
}

fn to_pascal_case(text: &str) -> String {
    const DELIMITER: char = '_';
    text.split(DELIMITER)
        .map(|f| {
            let mut chars = f.to_lowercase().chars().collect::<Vec<char>>();
            chars[0] = chars[0].to_ascii_uppercase();
            chars.into_iter().collect::<String>()
        })
        .collect::<Vec<String>>()
        .join("")
}

fn schema_type_to_rust(rust_type: &str) -> String {
    match rust_type {
        "string" => "String".to_string(),
        "integer" => "i32".to_string(),
        "boolean" => "bool".to_string(),
        other => to_pascal_case(other),
    }
}

#[derive(Debug)]
struct StructData {
    name: String,
    fields: Vec<Field>,
}

impl StructData {
    fn from_value(parent_key: &str, data: &Value) -> Option<Self> {
        let fields = data
            .get("properties")?
            .as_object()?
            .iter()
            .map(|(name, data)| Field::from_value(&parent_key, &name, data))
            .collect::<Option<Vec<Field>>>()?;

        Some(StructData {
            name: parent_key.to_string(),
            fields,
        })
    }

    fn to_code(self) -> String {
        let mut code = String::new();

        code.push_str(&format!("{DEFAULT_DERIVES}\n"));
        code.push_str(&format!(
            "pub struct {} {}\n",
            to_pascal_case(&self.name),
            CURLY[0]
        ));

        // we generate this here because of &self but add it on at the end
        let enum_arg_code = self.get_arg_enums();

        let field_len = self.fields.len();
        for (i, field) in self.fields.into_iter().enumerate() {
            code.push_str(&field.to_code());

            if i != (field_len - 1) {
                code.push_str(",\n");
            }
        }

        code.push('\n');
        code.push(CURLY[1]);
        code.push('\n');

        if !enum_arg_code.is_empty() {
            code.push_str(&enum_arg_code);
        }

        code
    }

    fn get_arg_enums(&self) -> String {
        let mut code = String::new();

        for field in &self.fields {
            // common derives
            if !(field.type_union.is_some() || field.type_enum.is_some()) {
                continue;
            }

            code.push_str(&format!("{DEFAULT_DERIVES}\n"));

            if let Some(union) = &field.type_enum {
                code.push_str(&format!(
                    "pub enum {} {}\n",
                    field.rust_type.inner(),
                    CURLY[0]
                ));
                for (i, variant) in union.iter().enumerate() {
                    code.push_str(&format!("{IDENTATION}#[serde(rename = \"{}\")]\n", variant));
                    code.push_str(IDENTATION);
                    code.push_str(&to_pascal_case(&variant));

                    if i != (union.len() - 1) {
                        code.push_str(",\n");
                    }
                }
            } else if let Some(_enum) = &field.type_union {
                code.push_str("#[serde(untagged)]\n");

                code.push_str(&format!(
                    "pub enum {} {}\n",
                    field.rust_type.inner(),
                    CURLY[0]
                ));

                for (i, variant) in _enum.iter().enumerate() {
                    code.push_str(&format!(
                        "{IDENTATION}{}({})",
                        to_pascal_case(&variant),
                        schema_type_to_rust(&variant)
                    ));

                    if i != (_enum.len() - 1) {
                        code.push_str(",\n");
                    }
                }
            }

            // common ending
            code.push('\n');
            code.push(CURLY[1]);
            code.push('\n');
        }

        code
    }
}

#[derive(Debug, Clone)]
struct Field {
    name: String,
    rust_type: RustType,
    attribute: Option<String>,
    type_union: Option<Vec<String>>,
    type_enum: Option<Vec<String>>,
}

impl Field {
    fn from_value(struct_key: &str, parent_key: &str, data: &Value) -> Option<Self> {
        Some(Field {
            name: parent_key.to_string(),
            rust_type: RustType::new(&data, Some(struct_key), Some(parent_key))?,
            attribute: None,
            type_union: if data.get("type").unwrap_or(&Value::Null).is_array() {
                Some(
                    data.get("type")?
                        .as_array()?
                        .iter()
                        .map(|s| s.as_str().unwrap().to_string())
                        .collect(),
                )
            } else {
                None
            },
            type_enum: data.get("enum").map(|e| {
                e.as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s.as_str().unwrap().to_string())
                    .collect()
            }),
        })
    }

    fn to_code(self) -> String {
        let mut field = String::new();

        if let Some(attr) = self.attribute {
            field.push_str(&format!("{IDENTATION}{attr}\n"));
        }

        let (field_name, name_modified) = field_case(&self.name);
        if name_modified {
            field.push_str(&format!(
                "{IDENTATION}#[serde(rename = \"{}\")]\n",
                self.name
            ));
        }

        field.push_str(&format!(
            "{IDENTATION}pub {}: {}",
            field_name,
            self.rust_type.inner()
        ));

        field
    }
}

#[derive(Debug)]
struct EnumData {
    name: String,
    variants: Vec<String>,
    enum_type: String,
    attribute: Option<String>,
}

impl EnumData {
    fn from_value(parent_key: &str, data: &Value) -> Option<Self> {
        let variants = data
            .get("enum")?
            .as_array()?
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect::<Vec<String>>();
        let enum_type = data.get("type")?.as_str()?.to_string();

        Some(EnumData {
            name: parent_key.to_string(),
            variants,
            enum_type,
            attribute: None,
        })
    }

    fn to_code(self) -> String {
        let mut code = String::new();

        if self.enum_type != "string" {
            unimplemented!(
                "Any other rust type other than string is currently not supported, if this panic was naturally triggered via the autogenerated schema then this will need to be implemented."
            );
        }

        code.push_str(&format!("{DEFAULT_DERIVES}\n"));
        if let Some(attr) = self.attribute {
            code.push_str(&attr);
            code.push('\n');
        }
        code.push_str(&format!(
            "pub enum {} {}\n",
            to_pascal_case(&self.name),
            CURLY[0]
        ));

        let variant_len = self.variants.len();
        for (i, variant) in self.variants.into_iter().enumerate() {
            code.push_str(&format!("{IDENTATION}#[serde(rename = \"{variant}\")]\n"));
            code.push_str(&format!("{IDENTATION}{}", to_pascal_case(&variant)));

            if i != (variant_len - 1) {
                code.push_str(",\n");
            }
        }

        code.push('\n');
        code.push(CURLY[1]);
        code.push('\n');

        code
    }
}

#[derive(Debug)]
struct FunctionData {
    doc: String,
    name: String,
    endpoint: String,
    function_type: FunctionType,
    params: Vec<FunctionParam>,
    return_type: RustType,
}

#[derive(Debug, Clone)]
struct FunctionParam {
    raw_name: String,
    name: String,
    rust_type: RustType,
}

#[derive(Debug)]
enum FunctionType {
    Request,
    Notification,
}

impl FunctionData {
    fn from_value(data: &Value) -> Option<Self> {
        let doc = data.get("description")?.as_str()?.to_string();
        let name = data
            .get("name")?
            .as_str()?
            .trim_start_matches("minecraft:")
            .replace('/', "_");
        let endpoint = data.get("name")?.as_str()?.to_string();

        let function_type = if name.starts_with("notification") {
            FunctionType::Notification
        } else {
            FunctionType::Request
        };
        println!("{doc:?}, {name:?}, {endpoint:?}, {function_type:?}");

        let (params, return_type) = match function_type {
            FunctionType::Request => {
                let params: Vec<FunctionParam> = data
                    .get("params")?
                    .as_array()?
                    .iter()
                    .map(|p| FunctionParam::from_value(p))
                    .collect::<Option<Vec<FunctionParam>>>()?;
                let return_type = if let Some(return_data) = data.get("result") {
                    RustType::new(return_data.get("schema")?, None, None)?
                } else {
                    RustType::new_empty()
                };

                (params, return_type)
            }
            FunctionType::Notification => {
                // the param in notifications IS the return type since they dont have any params
                (
                    vec![],
                    if let Some(result) = data.get("params")?.as_array()?.get(0) {
                        RustType::new(result.as_object()?.get("schema")?, None, None)?
                    } else {
                        RustType::new_empty()
                    },
                )
            }
        };

        Some(FunctionData {
            doc,
            name,
            endpoint,
            function_type,
            params,
            return_type,
        })
    }

    fn to_code(self) -> String {
        let mut code = String::new();

        let mut args = vec!["&self".to_string()];
        args.append(
            &mut self
                .params
                .clone()
                .into_iter()
                .map(|s| s.to_code())
                .collect::<Vec<String>>(),
        );

        code.push_str(&format!("{IDENTATION}/// {}\n", self.doc));
        code.push_str(&format!(
            "{IDENTATION}pub async fn {}({}) -> Result<",
            field_case(&self.name).0,
            args.join(", ")
        ));

        match self.function_type {
            FunctionType::Notification => {
                code.push_str(&format!(
                    "impl Stream<Item = Option<std::result::Result<Vec<{}>, serde_json::Error>>>",
                    self.return_type.inner()
                ));
            }
            FunctionType::Request => {
                code.push_str(self.return_type.inner());
            }
        }
        code.push_str(&format!("> {}\n", CURLY[0]));

        match self.function_type {
            FunctionType::Notification => {
                code.push_str(&format!(
                    "{FN_IDENTATION}self.0.subscribe(\"{}\").await",
                    self.endpoint
                ));
            }
            FunctionType::Request => {
                if self.params.is_empty() {
                    code.push_str(&format!(
                        "{FN_IDENTATION}self.0.request(\"{}\", None).await",
                        self.endpoint
                    ));
                } else {
                    // move all args into a hashmap
                    code.push_str(
                        &format!("{FN_IDENTATION}let mut map: std::collections::HashMap<String, serde_json::Value> = std::collections::HashMap::new();\n"),
                    );
                    for param in self.params {
                        code.push_str(&format!(
                            "{FN_IDENTATION}map.insert(\"{}\".to_string(), serde_json::to_value(&{})?);\n",
                            param.raw_name, param.name
                        ));
                    }

                    code.push_str(&format!(
                        "{FN_IDENTATION}self.0.request(\"{}\", Some(map)).await",
                        self.endpoint
                    ));
                }
            }
        };

        code.push_str(&format!("\n{IDENTATION}{}\n", CURLY[1]));

        code
    }
}

impl FunctionParam {
    fn from_value(data: &Value) -> Option<Self> {
        let raw_name = data.get("name")?.as_str()?.to_string();
        let name = if raw_name.starts_with("type") || raw_name.starts_with("use") {
            format!("_{}", raw_name)
        } else {
            raw_name.to_string()
        };
        let rust_type = RustType::new(data.get("schema")?, None, None)?;

        Some(FunctionParam {
            raw_name,
            name,
            rust_type,
        })
    }

    fn to_code(self) -> String {
        format!("{}: {}", self.name, self.rust_type.inner())
    }
}
