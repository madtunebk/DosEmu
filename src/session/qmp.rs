use serde_json::{Value, json};
use std::error::Error;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// One QMP connection shared by the input and video threads; lock per command.
pub type SharedQmp = Arc<Mutex<QmpClient>>;

pub struct QmpClient {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}

impl QmpClient {
    pub fn connect(socket_path: &Path) -> Result<Self, Box<dyn Error>> {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut last_error = None;

        loop {
            match UnixStream::connect(socket_path) {
                Ok(stream) => {
                    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                    let reader = BufReader::new(stream.try_clone()?);
                    let mut client = Self { stream, reader };

                    match client.read_message() {
                        Ok(_) => {
                            client.execute("qmp_capabilities", None)?;
                            return Ok(client);
                        }
                        Err(err) => {
                            last_error = Some(err);
                        }
                    }
                }
                Err(err) => {
                    last_error = Some(Box::new(err));
                }
            }

            if Instant::now() >= deadline {
                let message = last_error
                    .map(|err| err.to_string())
                    .unwrap_or_else(|| format!("Timed out connecting to QMP socket at {}", socket_path.display()));
                return Err(message.into());
            }

            thread::sleep(Duration::from_millis(200));
        }
    }

    fn read_message(&mut self) -> Result<Value, Box<dyn Error>> {
        let mut line = String::new();
        self.reader.read_line(&mut line)?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return self.read_message();
        }

        let message: Value = serde_json::from_str(trimmed)?;
        if message.get("event").is_some() {
            return self.read_message();
        }

        Ok(message)
    }

    pub fn execute(&mut self, command: &str, arguments: Option<Value>) -> Result<Value, Box<dyn Error>> {
        let mut payload = json!({ "execute": command });
        if let Some(args) = arguments {
            payload["arguments"] = args;
        }

        self.stream.write_all(payload.to_string().as_bytes())?;
        self.stream.write_all(b"\n")?;
        self.stream.flush()?;

        loop {
            let response = self.read_message()?;
            if response.get("error").is_some() {
                return Err(format!("QMP error: {}", response["error"]).into());
            }
            if let Some(return_value) = response.get("return") {
                return Ok(return_value.clone());
            }
        }
    }

    pub fn query_status(&mut self) -> Result<String, Box<dyn Error>> {
        let result = self.execute("query-status", None)?;
        let status = result
            .get("status")
            .and_then(Value::as_str)
            .ok_or("query-status returned no status field")?;
        Ok(status.to_string())
    }

    pub fn hmp_send_key(&mut self, key: &str) -> Result<Value, Box<dyn Error>> {
        let args = json!({ "command-line": format!("sendkey {key}") });
        self.execute("human-monitor-command", Some(args))
    }
}
