// TODO figure out interactive commands out (anything that needs user input)
// TODO this probably has to be a daemon that runs in the background and listens for commands
// TODO give user the option to start server in vebose mode (config file? command line arg?)
// TODO give the user the option to choose what shell to use
// TODO PTY?

use broadcast_protocol::{CommandRequest, CommandResponse, PORT};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::Level;

#[derive(thiserror::Error, Debug)]
pub enum ServerError {
    #[error("IO Error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("MPSC Send Error: {0}")]
    MpscSendError(#[from] mpsc::error::SendError<CommandResponse>),
    #[error("Join Error: {0}")]
    JoinError(#[from] tokio::task::JoinError),
    #[error("Protocol Error: {0}")]
    ProtocolError(#[from] broadcast_protocol::ProtcolError),
    #[error("Invalid Path Error: {0}")]
    InvalidPathError(String),
}

pub type ServerResult<T> = Result<T, ServerError>;

#[tokio::main]
async fn main() -> ServerResult<()> {
    // TODO change this and make configurable
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();

    let addr = format!("127.0.0.1:{}", PORT);
    let listener = TcpListener::bind(&addr).await?;

    tracing::info!("Listening on {}", addr);

    loop {
        match listener.accept().await {
            Ok((socket, addr)) => {
                tracing::info!("Accepted connection from {}", addr);
                tokio::spawn(async move {
                    if let Err(e) = handle_client(socket).await {
                        tracing::error!("Error handling client {}: {}", addr, e);
                    }
                });
            }
            Err(e) => {
                tracing::error!("Failed to accept connection: {}", e);
            }
        }
    }
}

async fn handle_client(mut socket: TcpStream) -> ServerResult<()> {
    let request: CommandRequest = broadcast_protocol::decode_msg(&mut socket).await?;

    tracing::info!("Executing: {} in {}", request.command, request.working_dir);

    let wsl_path = convert_win_to_wsl_path(&request.working_dir)?;

    let (tx, mut rx) = mpsc::unbounded_channel::<CommandResponse>();

    let cmd = request.command.clone();

    tokio::spawn(async move {
        let _ = execute_command(&cmd, &wsl_path, tx).await;
    });

    while let Some(response) = rx.recv().await {
        let encoded = broadcast_protocol::encode_msg(&response)?;
        socket.write_all(&encoded).await?;

        if matches!(
            response,
            CommandResponse::Exit(_) | CommandResponse::Error(_)
        ) {
            break;
        }
    }

    Ok(())
}

// TODO user should be able to choose shell (default sh)
// Using different shells _might_ require different command formats, haven't thought enough about this yet
// So we might up ending with a list of supported shells that the user can choose from
async fn execute_command(
    cmd: &str,
    working_dir: &str,
    tx: mpsc::UnboundedSender<CommandResponse>,
) -> ServerResult<()> {
    let mut child = match Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            tx.send(CommandResponse::Error(format!("{}", e)))?;
            return Err(e.into());
        }
    };

    let Some(stdout) = child.stdout.take() else {
        let msg = "Failed to capture stdout".to_string();
        tx.send(CommandResponse::Error(msg.clone()))?;
        return Err(ServerError::IoError(std::io::Error::new(
            std::io::ErrorKind::Other,
            msg,
        )));
    };

    let Some(stderr) = child.stderr.take() else {
        let msg = "Failed to capture stderr".to_string();
        tx.send(CommandResponse::Error(msg.clone()))?;
        return Err(ServerError::IoError(std::io::Error::new(
            std::io::ErrorKind::Other,
            msg,
        )));
    };

    let tx_clone = tx.clone();
    let stdout_task = tokio::spawn(stream_output(stdout, tx_clone, true));

    let tx_clone = tx.clone();
    let stderr_task = tokio::spawn(stream_output(stderr, tx_clone, false));

    stdout_task.await??;
    stderr_task.await??;

    let status = child.wait().await?;
    let exit_code = status.code().unwrap_or(-1);

    tx.send(CommandResponse::Exit(exit_code))?;

    Ok(())
}

async fn stream_output(
    mut reader: impl AsyncReadExt + Unpin,
    tx: mpsc::UnboundedSender<CommandResponse>,
    is_stdout: bool,
) -> ServerResult<()> {
    let mut buffer = vec![0u8; 8192];

    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break, // EOF
            Ok(n) => {
                let response = if is_stdout {
                    broadcast_protocol::CommandResponse::Stdout(buffer[..n].to_vec())
                } else {
                    broadcast_protocol::CommandResponse::Stderr(buffer[..n].to_vec())
                };

                tx.send(response)?;
            }
            Err(e) => return Err(e.into()),
        }
    }

    Ok(())
}

/// Converts a given windows path to a wsl path, E.g., "C:\Users\Username" -> "/mnt/c/Users/Username"
fn convert_win_to_wsl_path(win_path: &str) -> ServerResult<String> {
    let mut chars = win_path.chars();
    let Some(next) = chars.next() else {
        return Err(ServerError::InvalidPathError(format!(
            "Empty path provided"
        )));
    };
    let drive_letter = next.to_ascii_lowercase();
    let rest_of_path: String = chars.collect();
    let rest_of_path = rest_of_path.replace('\\', "/");
    let converted_path = format!(
        "/mnt/{}/{}",
        drive_letter,
        rest_of_path.trim_start_matches(':')
    );

    if !PathBuf::from(&converted_path).exists() {
        return Err(ServerError::InvalidPathError(format!(
            "Converted path '{}' is invalid",
            converted_path
        )));
    };

    Ok(converted_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_conversion() {
        assert_eq!(
            convert_win_to_wsl_path("C:\\Users\\Username").unwrap(),
            "/mnt/c/Users/Username"
        );
        assert_eq!(
            convert_win_to_wsl_path("D:\\Projects\\Test").unwrap(),
            "/mnt/d/Projects/Test"
        );

        assert!(convert_win_to_wsl_path("").is_err());
    }
}
