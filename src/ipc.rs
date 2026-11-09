use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

pub const MAX_MESSAGE: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub details: Vec<String>,
}

impl ApiError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: Vec::new(),
        }
    }

    pub fn with_details(mut self, details: Vec<String>) -> Self {
        self.details = details;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ApiError>,
}

impl Response {
    pub fn ok(id: u64, value: impl Serialize) -> Self {
        Self {
            id,
            ok: true,
            result: Some(serde_json::to_value(value).unwrap_or(Value::Null)),
            error: None,
        }
    }

    pub fn error(id: u64, error: ApiError) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(error),
        }
    }
}

pub fn write_message(stream: &mut UnixStream, message: &impl Serialize) -> Result<(), String> {
    let mut text = serde_json::to_string(message).map_err(|e| format!("IPC serialization: {e}"))?;
    if text.len() + 1 > MAX_MESSAGE {
        return Err("IPC message too large".to_string());
    }
    text.push('\n');
    stream
        .write_all(text.as_bytes())
        .map_err(|e| format!("IPC write: {e}"))
}

pub fn read_message<T: for<'de> Deserialize<'de>>(
    reader: &mut BufReader<&UnixStream>,
) -> Result<T, String> {
    let mut line = String::new();
    let read = reader
        .by_ref()
        .take((MAX_MESSAGE + 1) as u64)
        .read_line(&mut line)
        .map_err(|e| format!("IPC read: {e}"))?;
    if read == 0 {
        return Err("IPC connection closed".to_string());
    }
    if line.len() > MAX_MESSAGE {
        return Err("IPC message too large".to_string());
    }
    serde_json::from_str(&line).map_err(|e| format!("invalid IPC JSON: {e}"))
}

#[derive(Debug)]
pub struct Client {
    stream: UnixStream,
    next_id: u64,
}

impl Client {
    pub fn connect(path: &Path) -> Result<Self, ApiError> {
        let stream = UnixStream::connect(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                ApiError::new(
                    "permission_denied",
                    format!(
                        "socket access denied ({}); this session may lack the neosense group; log out/in or run `newgrp neosense`",
                        path.display()
                    ),
                )
                .with_details(vec![
                    "group members are authorised; the socket must remain 0660".to_string(),
                ])
            } else {
                ApiError::new(
                    "ipc_connect",
                    format!("supervisor unavailable ({}): {e}", path.display()),
                )
            }
        })?;
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .map_err(|e| ApiError::new("ipc_config", e.to_string()))?;
        stream
            .set_write_timeout(Some(std::time::Duration::from_secs(10)))
            .map_err(|e| ApiError::new("ipc_config", e.to_string()))?;
        Ok(Self { stream, next_id: 1 })
    }

    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, ApiError> {
        let id = self.next_id;
        self.next_id += 1;
        let request = Request {
            id,
            method: method.to_string(),
            params,
        };
        write_message(&mut self.stream, &request).map_err(|e| ApiError::new("ipc_write", e))?;
        let mut reader = BufReader::new(&self.stream);
        let response: Response =
            read_message(&mut reader).map_err(|e| ApiError::new("ipc_read", e))?;
        if response.id != id {
            return Err(ApiError::new(
                "ipc_protocol",
                format!(
                    "IPC response id mismatch: expected {id}, got {}",
                    response.id
                ),
            ));
        }
        if response.ok {
            Ok(response.result.unwrap_or(Value::Null))
        } else {
            Err(response
                .error
                .unwrap_or_else(|| ApiError::new("ipc_error", "response without error")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn oversized_message_is_rejected_before_unbounded_allocation() {
        let (left, right) = UnixStream::pair().unwrap();
        let mut writer = right;
        writer.write_all(&vec![b'x'; MAX_MESSAGE + 1]).unwrap();
        writer.write_all(b"\n").unwrap();
        let mut reader = BufReader::new(&left);
        let error = read_message::<Request>(&mut reader).unwrap_err();
        assert!(error.contains("too large"));
    }
    #[test]
    fn permission_denied_is_typed_and_actionable() {
        if unsafe { libc::geteuid() } == 0 {
            return; // root bypasses discretionary socket permissions
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, {
            use std::os::unix::fs::PermissionsExt;
            std::fs::Permissions::from_mode(0o000)
        })
        .unwrap();
        let error = Client::connect(&path).unwrap_err();
        assert_eq!(error.code, "permission_denied");
        assert!(error.message.contains("neosense"));
        drop(listener);
    }

    #[test]
    fn response_id_mismatch_is_a_protocol_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sock");
        let listener = UnixListener::bind(&path).unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let request: Request = read_message(&mut reader).unwrap();
            write_message(
                &mut stream,
                &Response::ok(request.id + 1, serde_json::json!({"pong": true})),
            )
            .unwrap();
        });
        let mut client = Client::connect(&path).unwrap();
        let error = client.call("ping", serde_json::json!({})).unwrap_err();
        assert_eq!(error.code, "ipc_protocol");
        handle.join().unwrap();
    }
    #[test]
    fn request_round_trip_over_socketpair() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sock");
        let listener = UnixListener::bind(&path).unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let request: Request = read_message(&mut reader).unwrap();
            assert_eq!(request.method, "ping");
            write_message(
                &mut stream,
                &Response::ok(request.id, serde_json::json!({"pong": true})),
            )
            .unwrap();
        });

        let mut client = Client::connect(&path).unwrap();
        let value = client.call("ping", serde_json::json!({})).unwrap();
        assert_eq!(value["pong"], true);
        handle.join().unwrap();
    }
}
