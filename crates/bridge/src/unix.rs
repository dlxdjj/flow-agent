use flow_agent_core::{BridgeRequest, BridgeResponse};
use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("bridge I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("bridge protocol failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("bridge closed without a response")]
    MissingResponse,
}

pub fn default_socket_path() -> PathBuf {
    if let Some(root) = env::var_os("FLOW_AGENT_HOME") {
        return PathBuf::from(root).join("run/bridge.sock");
    }
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".flow-agent/run/bridge.sock")
}

pub struct BridgeClient {
    socket_path: PathBuf,
}

impl BridgeClient {
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    pub fn send(
        &self,
        request: &BridgeRequest,
        timeout: Duration,
    ) -> Result<Option<BridgeResponse>, BridgeError> {
        let mut stream = UnixStream::connect(&self.socket_path)?;
        stream.set_write_timeout(Some(Duration::from_millis(200)))?;
        stream.set_read_timeout(Some(timeout))?;
        serde_json::to_writer(&mut stream, request)?;
        stream.write_all(b"\n")?;
        stream.flush()?;

        if !request.needs_reply {
            return Ok(None);
        }

        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        if line.trim().is_empty() {
            return Err(BridgeError::MissingResponse);
        }
        Ok(Some(serde_json::from_str(&line)?))
    }
}

pub struct BridgeListener {
    listener: UnixListener,
    socket_path: PathBuf,
}

impl BridgeListener {
    pub fn bind(socket_path: impl Into<PathBuf>) -> Result<Self, BridgeError> {
        let socket_path = socket_path.into();
        if let Some(parent) = socket_path.parent() {
            let created_parent = !parent.exists();
            fs::create_dir_all(parent)?;
            // Never chmod an arbitrary existing directory supplied via --socket
            // (for example /private/tmp). Directories created by flow-agent are
            // private from their first usable moment.
            if created_parent {
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
            }
        }
        if socket_path.exists() {
            fs::remove_file(&socket_path)?;
        }
        let listener = UnixListener::bind(&socket_path)?;
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
        Ok(Self {
            listener,
            socket_path,
        })
    }

    pub fn incoming(&self) -> impl Iterator<Item = io::Result<UnixStream>> + '_ {
        self.listener.incoming()
    }

    pub fn read_request(stream: &mut UnixStream) -> Result<BridgeRequest, BridgeError> {
        let mut line = String::new();
        BufReader::new(stream.try_clone()?).read_line(&mut line)?;
        if line.trim().is_empty() {
            return Err(BridgeError::MissingResponse);
        }
        Ok(serde_json::from_str(&line)?)
    }

    pub fn write_response(
        stream: &mut UnixStream,
        response: &BridgeResponse,
    ) -> Result<(), BridgeError> {
        serde_json::to_writer(&mut *stream, response)?;
        stream.write_all(b"\n")?;
        stream.flush()?;
        Ok(())
    }
}

impl Drop for BridgeListener {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
    }
}

#[cfg(test)]
fn socket_is_private(path: &Path) -> io::Result<bool> {
    Ok(fs::metadata(path)?.permissions().mode() & 0o777 == 0o600)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flow_agent_core::{Decision, Provider};
    use std::thread;

    fn temp_socket(name: &str) -> PathBuf {
        env::temp_dir().join(format!("flow-agent-{name}-{}.sock", std::process::id()))
    }

    #[test]
    fn round_trips_a_blocking_request() {
        let path = temp_socket("roundtrip");
        let listener = BridgeListener::bind(&path).unwrap();
        assert!(socket_is_private(&path).unwrap());

        let request = BridgeRequest::from_hook(
            Provider::Claude,
            serde_json::json!({
                "hook_event_name": "PermissionRequest",
                "session_id": "session-1"
            }),
        );
        let expected_id = request.id;
        let server = thread::spawn(move || {
            let mut stream = listener.incoming().next().unwrap().unwrap();
            let request = BridgeListener::read_request(&mut stream).unwrap();
            BridgeListener::write_response(
                &mut stream,
                &BridgeResponse::decided(request.id, Decision::Allow),
            )
            .unwrap();
        });

        let response = BridgeClient::new(path)
            .send(&request, Duration::from_secs(1))
            .unwrap()
            .unwrap();
        server.join().unwrap();

        assert_eq!(response.request_id, expected_id);
        assert_eq!(response.decision, Some(Decision::Allow));
    }
}
