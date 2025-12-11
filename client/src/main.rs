use std::{
    env,
    io::{self, Write},
};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tracing::Level;

// TODO have install command, that downloads the server trough "broadcast setup"
// TODO give the user a command to setup aliases in their shell (broadcast install -c "command" -a "alias")
// The user should be able to choose the wsl distribution to install to (nice menu for that)
// TODO figure interactive commands out (anything that needs user input)
// TODO have edit config command so config can be edited from windows
use broadcast_protocol::{CommandRequest, CommandResponse, PORT};
use clap::Parser;

#[derive(clap::Parser, Debug)]
#[command(name = "broadcast", about = "Broadcast commands to WSL")]
#[command(author, version, about, long_about = None)]
#[command(arg_required_else_help = true)]
struct Args {
    #[arg(
        required = true,
        trailing_var_arg = true,
        allow_hyphen_values = true,
        help = "The command to broadcast"
    )]
    command: Vec<String>,
}

#[derive(thiserror::Error, Debug)]
enum ClientErrors {
    #[error("IO Error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Protocol Error: {0}")]
    ProtocolError(#[from] broadcast_protocol::ProtcolError),
}

type ClientResult<T> = Result<T, ClientErrors>;

#[tokio::main]
async fn main() -> ClientResult<()> {
    let args = Args::parse();

    tracing_subscriber::fmt().with_max_level(Level::INFO).init();

    let cmd = args.command.join(" ");

    let cwd = env::current_dir()?.to_string_lossy().to_string();
    // everytihng followed after the first

    let addr = format!("127.0.0.1:{}", PORT);
    let mut stream = TcpStream::connect(&addr).await?;

    let request = CommandRequest {
        command: cmd.clone(),
        working_dir: cwd,
    };

    let msg = broadcast_protocol::encode_msg(&request)?;
    stream.write_all(&msg).await?;

    let exit_code = handle_response(&mut stream).await?;
    std::process::exit(exit_code);
}

async fn handle_response(stream: &mut TcpStream) -> ClientResult<i32> {
    let stdout = io::stdout();
    let stderr = io::stderr();

    let mut stdout_handle = stdout.lock();
    let mut stderr_handle = stderr.lock();

    loop {
        match broadcast_protocol::decode_msg::<CommandResponse>(stream).await {
            Ok(response) => match response {
                CommandResponse::Stdout(data) => {
                    stdout_handle.write_all(&data)?;
                    stdout_handle.flush()?;
                }
                CommandResponse::Stderr(data) => {
                    stderr_handle.write_all(&data)?;
                    stderr_handle.flush()?;
                }
                CommandResponse::Exit(code) => {
                    return Ok(code);
                }
                CommandResponse::Error(msg) => {
                    tracing::error!("Server Error: {}", msg);
                    return Ok(1);
                }
            },
            Err(e) => {
                if e.to_string().contains("UnexpectedEof") {
                    tracing::error!("Connection closed unexpectedly");
                    return Ok(1);
                }
                return Err(e.into());
            }
        }
    }
}
