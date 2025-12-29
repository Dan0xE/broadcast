use std::{
    env,
    io::{IsTerminal, Write, stdout},
};
use termina::Terminal as _;
use tokio::net::TcpStream;
use tokio::{io::AsyncWriteExt, task::JoinHandle};
use tracing::Level;

// TODO have install command, that downloads the server trough "broadcast setup"
// TODO give the user a command to setup aliases in their shell (broadcast install -c "command" -a "alias")
// The user should be able to choose the wsl distribution to install to (nice menu for that)
// TODO have edit config command so config can be edited from windows
use broadcast_protocol::{
    ClientMessage, CommandRequest, CommandResponse, PORT, decode_msg, encode_msg,
};
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
    #[arg(short, long, help = "Enable verbose output")]
    verbose: bool,
    #[arg(short, long, help = "Enable debug logging to file")]
    debug: bool,
    // shell, can overwrite default / config value
}

#[derive(thiserror::Error, Debug)]
enum ClientError {
    #[error("IO Error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Protocol Error: {0}")]
    Protocol(#[from] broadcast_protocol::ProtocolError),
    #[error("Join Error: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("Log Setup Error: {0}")]
    LogSetup(#[from] tracing::subscriber::SetGlobalDefaultError),
}

type ClientResult<T> = Result<T, ClientError>;

#[tokio::main]
async fn main() -> ClientResult<()> {
    let args = Args::parse();

    let _log_guard = setup_logging(args.debug, args.verbose)?;

    let cmd = args.command.join(" ");

    let cwd = env::current_dir()?.to_string_lossy().to_string();

    let stdin_is_tty = std::io::stdin().is_terminal();

    let addr = format!("127.0.0.1:{}", PORT);
    let mut stream = TcpStream::connect(&addr).await?;

    // disable Nagle's algorithm, we need to disable TCP buffering for interactive commands,
    // otherwise we get weird issues
    stream.set_nodelay(true)?;

    let terminal_size = if stdin_is_tty {
        let terminal = termina::PlatformTerminal::new()?;
        terminal.get_dimensions().ok().map(|ws| (ws.cols, ws.rows))
    } else {
        None
    };

    let request = CommandRequest {
        command: cmd.clone(),
        working_dir: cwd,
        terminal_size,
    };

    let msg = broadcast_protocol::encode_msg(&request)?;
    stream.write_all(&msg).await?;

    let exit_code = handle_response(stream, stdin_is_tty).await?;
    std::process::exit(exit_code);
}

fn setup_logging(
    debug: bool,
    verbose: bool,
) -> ClientResult<Option<tracing_appender::non_blocking::WorkerGuard>> {
    let dbg = debug || verbose;
    let level = if dbg { Level::DEBUG } else { Level::INFO };

    if dbg {
        let file_appender = tracing_appender::rolling::daily("logs", "broadcast-client.log");
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

        tracing_subscriber::fmt()
            .with_max_level(level)
            .with_writer(non_blocking)
            .with_ansi(false)
            .init();

        tracing::debug!("Debug logging enabled");
        return Ok(Some(guard));
    }

    tracing_subscriber::fmt().with_max_level(level).init();
    Ok(None)
}

struct RawModeGuard {
    enabled: bool,
}

impl RawModeGuard {
    fn new(enable: bool) -> ClientResult<Self> {
        if enable {
            crossterm::terminal::enable_raw_mode()?;
        }
        Ok(Self { enabled: enable })
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.enabled {
            use crossterm::{
                cursor, execute,
                terminal::{self, Clear, ClearType},
            };

            let mut stdout = stdout();

            if let Err(e) = terminal::disable_raw_mode() {
                tracing::error!("Failed to disable raw mode: {}", e);
            }

            if let Err(e) = execute!(
                stdout,
                terminal::LeaveAlternateScreen,
                cursor::Show,
                Clear(ClearType::All),
            ) {
                tracing::error!("Failed to reset terminal state: {}", e);
            }
        }
    }
}

async fn handle_response(stream: TcpStream, stdin_is_tty: bool) -> ClientResult<i32> {
    // raw mode is disabled even if we return early or panic
    let _raw_mode_guard = RawModeGuard::new(stdin_is_tty)?;

    let terminal = if stdin_is_tty {
        Some(termina::PlatformTerminal::new()?)
    } else {
        None
    };

    let (mut stream_read, mut stream_write) = stream.into_split();

    let output_task: JoinHandle<ClientResult<i32>> = tokio::spawn(async move {
        let mut stdout = stdout();

        loop {
            match decode_msg::<CommandResponse>(&mut stream_read).await {
                Ok(response) => match response {
                    CommandResponse::Stdout(data) => {
                        stdout.write_all(&data)?;
                        stdout.flush()?;
                    }
                    CommandResponse::Exit(code) => return Ok(code),
                    CommandResponse::Error(e) => {
                        tracing::error!("Server error: {}", e);
                        return Ok(1);
                    }
                    // Stderr is merged
                    _ => {}
                },
                Err(_) => {
                    // Connection closed
                    tracing::warn!("Connection closed by server");
                    return Ok(1);
                }
            }
        }
    });

    let input_task = if let Some(term) = terminal {
        tokio::task::spawn_blocking(move || {
            loop {
                let event = match term.read(|_| true) {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::error!("Error reading terminal event: {}", e);
                        break;
                    }
                };

                let result: ClientResult<()> = tokio::runtime::Handle::current().block_on(async {
                    match &event {
                        termina::Event::Key(key_event) => {
                            if key_event.kind != termina::event::KeyEventKind::Press {
                                return Ok(());
                            }

                            if key_event.code == termina::event::KeyCode::Char('c')
                                && key_event
                                    .modifiers
                                    .contains(termina::event::Modifiers::CONTROL)
                            {
                                let msg = encode_msg(&ClientMessage::Input(vec![3]))?;
                                stream_write.write_all(&msg).await?;
                                return Err(ClientError::Io(std::io::Error::new(
                                    std::io::ErrorKind::Interrupted,
                                    "Ctrl+C",
                                )));
                            }

                            if key_event.code == termina::event::KeyCode::Char('d')
                                && key_event
                                    .modifiers
                                    .contains(termina::event::Modifiers::CONTROL)
                            {
                                let msg = encode_msg(&ClientMessage::Eof)?;
                                stream_write.write_all(&msg).await?;
                                return Err(ClientError::Io(std::io::Error::new(
                                    std::io::ErrorKind::Interrupted,
                                    "Ctrl+D",
                                )));
                            }

                            if let Some(bytes) = event_to_bytes(&event) {
                                let msg = encode_msg(&ClientMessage::Input(bytes))?;
                                stream_write.write_all(&msg).await?;
                            }
                        }
                        termina::Event::WindowResized(ws) => {
                            tracing::debug!(
                                "Resize event: resized to {} cols and {} rows",
                                ws.cols,
                                ws.rows
                            );
                            let msg = encode_msg(&ClientMessage::Resize(ws.cols, ws.rows))?;
                            stream_write.write_all(&msg).await?;
                        }
                        event if event.is_escape() => {
                            tracing::debug!("Received escape sequence (VT response): {:?}", event);
                            if let Some(bytes) = event_to_bytes(event) {
                                tracing::debug!("Forwarding VT response bytes to PTY: {:?}", bytes);
                                let msg = encode_msg(&ClientMessage::Input(bytes))?;
                                stream_write.write_all(&msg).await?;
                            }
                        }
                        _ => {}
                    }
                    Ok(())
                });

                if let Err(e) = result {
                    tracing::warn!("Error handling input event: {}", e);
                    break;
                }
            }
            Ok::<_, ClientError>(())
        })
    } else {
        tokio::task::spawn_blocking(|| Ok::<_, ClientError>(()))
    };

    // wait for either task to complete
    // we want to ensure we don't hang if the server never responds after CTRL+C
    // if one branch completes, the other future is automatically dropped
    let exit_code = tokio::select! {
        result = output_task => {
            // task completed
            result??
        }
        result = input_task => {
            result??;
            tracing::debug!("Input task exited, returning exit code 130 (interrupted)");
            130 // SIGINT
        }
    };

    // cleanup gets done trough the RawModeGuard drop

    Ok(exit_code)
}

fn event_to_bytes(event: &termina::Event) -> Option<Vec<u8>> {
    use terminput::Encoding;

    match event {
        termina::Event::Key(key) => {
            let mut buf = [0u8; 32];
            let event = convert_key_event_to_terminput(key)?;
            let len = event.encode(&mut buf, Encoding::Xterm).ok()?;
            Some(buf[..len].to_vec())
        }
        _ => {
            tracing::debug!("Unhandled event type for encoding: {:?}", event);
            None
        }
    }
}

fn convert_key_event_to_terminput(key: &termina::event::KeyEvent) -> Option<terminput::Event> {
    use terminput::{
        Event, KeyCode, KeyEvent as TermInputKeyEvent, KeyEventKind, KeyEventState, KeyModifiers,
    };

    let code = match &key.code {
        termina::event::KeyCode::Char(c) => KeyCode::Char(*c),
        termina::event::KeyCode::Enter => KeyCode::Enter,
        termina::event::KeyCode::Backspace => KeyCode::Backspace,
        termina::event::KeyCode::Tab => KeyCode::Tab,
        termina::event::KeyCode::Escape => KeyCode::Esc,
        termina::event::KeyCode::BackTab => KeyCode::Tab,
        termina::event::KeyCode::Up => KeyCode::Up,
        termina::event::KeyCode::Down => KeyCode::Down,
        termina::event::KeyCode::Left => KeyCode::Left,
        termina::event::KeyCode::Right => KeyCode::Right,
        termina::event::KeyCode::Home => KeyCode::Home,
        termina::event::KeyCode::End => KeyCode::End,
        termina::event::KeyCode::PageUp => KeyCode::PageUp,
        termina::event::KeyCode::PageDown => KeyCode::PageDown,
        termina::event::KeyCode::Delete => KeyCode::Delete,
        termina::event::KeyCode::Insert => KeyCode::Insert,
        termina::event::KeyCode::Function(n) => KeyCode::F(*n),
        termina::event::KeyCode::Null => return None,
        termina::event::KeyCode::KeypadBegin => return None,
        termina::event::KeyCode::CapsLock => return None,
        termina::event::KeyCode::ScrollLock => return None,
        termina::event::KeyCode::NumLock => return None,
        termina::event::KeyCode::PrintScreen => return None,
        termina::event::KeyCode::Pause => return None,
        termina::event::KeyCode::Menu => return None,
        termina::event::KeyCode::Modifier(_) => return None,
        termina::event::KeyCode::Media(_) => return None,
    };

    let mut modifiers = KeyModifiers::empty();
    if key.modifiers.contains(termina::event::Modifiers::SHIFT) {
        modifiers |= KeyModifiers::SHIFT;
    }
    if key.modifiers.contains(termina::event::Modifiers::CONTROL) {
        modifiers |= KeyModifiers::CTRL;
    }
    if key.modifiers.contains(termina::event::Modifiers::ALT) {
        modifiers |= KeyModifiers::ALT;
    }

    let kind = match key.kind {
        termina::event::KeyEventKind::Press => KeyEventKind::Press,
        termina::event::KeyEventKind::Repeat => KeyEventKind::Repeat,
        termina::event::KeyEventKind::Release => KeyEventKind::Release,
    };

    Some(Event::Key(TermInputKeyEvent {
        code,
        modifiers,
        kind,
        state: KeyEventState::empty(),
    }))
}
