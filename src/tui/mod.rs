//! TUI module: full terminal user interface using ratatui.
//!
//! Replaces the old REPL with a scrollable, interactive interface.

pub mod app;
pub mod event;
pub mod widgets;

use std::io;
use std::path::PathBuf;

use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    widgets::Widget,
    Terminal,
};
use tokio::sync::mpsc;
// tui_textarea is used via widgets::input_area

use crate::config::Config;
use crate::monitor::SharedMetrics;

use self::app::{AgentPhase, App};
use self::event::{spawn_event_reader, AppEvent};
use self::widgets::{
    chat_panel::{ChatMessage, ChatPanel},
    input_area,
    status_bar::StatusBar,
};

/// Initialize the terminal and run the TUI event loop.
pub async fn run(config: Config, workspace_root: PathBuf, metrics: SharedMetrics) -> Result<()> {
    // Setup terminal
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    // Mouse capture ON for scroll wheel support.
    // To select/copy text: hold Shift while clicking/dragging (standard terminal behavior).
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Install panic hook to restore terminal
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(panic_info);
    }));

    // Create event channel
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<AppEvent>();

    // Spawn crossterm event reader (sends Key/Mouse/Tick events)
    let reader_rx = spawn_event_reader();
    // Forward reader events into our unified channel
    let fwd_tx = event_tx.clone();
    tokio::spawn(async move {
        let mut reader_rx = reader_rx;
        while let Some(evt) = reader_rx.recv().await {
            if fwd_tx.send(evt).is_err() {
                break;
            }
        }
    });

    // Create app
    let mut app = App::new(config, workspace_root, metrics, event_tx.clone());

    // Welcome message
    app.chat_messages.push(ChatMessage::System(format!(
        "Welcome to Allux v{} \u{2022} model: {} \u{2022} /help for commands",
        env!("CARGO_PKG_VERSION"),
        app.client.model
    )));

    // Create text area for input
    let mut textarea = input_area::new_textarea();

    // Main event loop
    loop {
        // Draw
        terminal.draw(|frame| {
            let area = frame.area();

            // Layout: status bar (1) | chat panel (fill) | input (3)
            let chunks = Layout::vertical([
                Constraint::Length(1),   // Status bar
                Constraint::Min(5),     // Chat panel
                Constraint::Length(3),   // Input area
            ])
            .split(area);

            // Status bar
            let status = StatusBar { app: &app };
            frame.render_widget(status, chunks[0]);

            // Store chat area for mouse mapping
            app.chat_area = chunks[1];

            // Chat panel
            let is_streaming = matches!(
                app.phase,
                AgentPhase::WaitingForLlm | AgentPhase::ExecutingTools
            );
            let chat = ChatPanel {
                messages: &app.chat_messages,
                streaming_text: &app.streaming_text,
                is_streaming,
                spinner_frame: app.spinner_frame,
                scroll_offset: app.scroll_offset,
                selection: app.selection_range(),
            };

            // Calculate view info for mouse mapping (before render consumes chat)
            let inner_height = chunks[1].height.saturating_sub(0) as usize; // no top/bottom border
            let (view_start, total_lines) = chat.calc_view(chunks[1].width, inner_height);
            app.chat_view_start = view_start;
            app.chat_total_lines = total_lines;

            frame.render_widget(chat, chunks[1]);

            // Input area
            frame.render_widget(&textarea, chunks[2]);

            // Autocomplete popup (rendered on top of chat panel, above input)
            let current_input = input_area::current_text(&textarea);
            let (completions, total_count) = input_area::get_completions(&current_input);
            if !completions.is_empty() {
                let popup = input_area::AutocompletePopup {
                    completions: &completions,
                    total_count,
                };
                popup.render(chunks[2], frame.buffer_mut());
            }
        })?;

        if app.should_quit {
            break;
        }

        // Wait for next event
        let Some(evt) = event_rx.recv().await else {
            break;
        };

        match evt {
            AppEvent::Key(key) => {
                // Ignore release events
                if key.kind == KeyEventKind::Release {
                    continue;
                }

                // Global shortcuts (work in any phase)
                match (key.code, key.modifiers) {
                    // Ctrl+C: double-tap to exit
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                        let input_text: String = textarea.lines().join("");
                        if input_text.is_empty() {
                            // No text in input → handle Ctrl+C for cancel/exit
                            if app.handle_ctrl_c() {
                                app.should_quit = true;
                            }
                            continue;
                        } else {
                            // Text in input → clear it, and reset Ctrl+C state
                            textarea.select_all();
                            textarea.cut();
                            app.clear_ctrl_c();
                            continue;
                        }
                    }
                    // Ctrl+D on empty input: quit
                    (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                        let text: String = textarea.lines().join("");
                        if text.is_empty() {
                            app.should_quit = true;
                            continue;
                        }
                    }
                    // Scroll: PageUp/Down (10 lines), Ctrl+Up/Down (3 lines)
                    (KeyCode::PageUp, _) => {
                        app.scroll_up(10);
                        continue;
                    }
                    (KeyCode::PageDown, _) => {
                        app.scroll_down(10);
                        continue;
                    }
                    (KeyCode::Up, KeyModifiers::CONTROL) => {
                        app.scroll_up(3);
                        continue;
                    }
                    (KeyCode::Down, KeyModifiers::CONTROL) => {
                        app.scroll_down(3);
                        continue;
                    }
                    // Escape: cancel current operation OR scroll to bottom
                    (KeyCode::Esc, _) => {
                        if app.phase != AgentPhase::Idle {
                            // Cancel streaming/tool execution
                            app.phase = AgentPhase::Idle;
                            app.streaming_text.clear();
                            app.chat_messages
                                .push(ChatMessage::System("Cancelled.".into()));
                            app.scroll_to_bottom();
                        } else {
                            // Scroll to bottom when idle
                            app.scroll_to_bottom();
                        }
                        continue;
                    }
                    _ => {}
                }

                // Any non-Ctrl+C key clears the "press again to exit" state
                app.clear_ctrl_c();

                // Input is ALWAYS active — user can type while LLM is thinking.
                // Submit is queued if busy; it will run after the current operation.
                match input_area::handle_key(&mut textarea, key) {
                    input_area::InputAction::Submit(text) => {
                        // Check for slash commands first
                        if app.handle_slash_command(&text) {
                            // slash command handled
                        } else if app.phase == AgentPhase::Idle {
                            // Send immediately
                            app.submit_user_input(text);
                        } else {
                            // Queue: show in chat, will be sent when current op finishes
                            app.enqueue_input(text);
                        }
                    }
                    input_area::InputAction::Quit => {
                        app.should_quit = true;
                    }
                    input_area::InputAction::Consumed => {}
                }
            }

            AppEvent::Mouse(mouse) => {
                match mouse.kind {
                    MouseEventKind::ScrollUp => app.scroll_up(3),
                    MouseEventKind::ScrollDown => app.scroll_down(3),
                    MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                        // Clear previous selection and start new one
                        app.clear_selection();
                        if mouse.row >= app.chat_area.y
                            && mouse.row < app.chat_area.y + app.chat_area.height
                        {
                            let is_double = app.start_selection(mouse.row, mouse.column);
                            if is_double {
                                // Double-click: select word (highlight only, no copy)
                                let is_streaming = matches!(
                                    app.phase,
                                    AgentPhase::WaitingForLlm | AgentPhase::ExecutingTools
                                );
                                let chat = ChatPanel {
                                    messages: &app.chat_messages,
                                    streaming_text: &app.streaming_text,
                                    is_streaming,
                                    spinner_frame: app.spinner_frame,
                                    scroll_offset: app.scroll_offset,
                                    selection: None,
                                };
                                let plain = chat.build_plain_lines(app.chat_area.width);
                                app.select_word_at(mouse.row, mouse.column, &plain);
                            }
                        }
                    }
                    MouseEventKind::Drag(crossterm::event::MouseButton::Left) => {
                        app.extend_selection(mouse.row, mouse.column);
                    }
                    MouseEventKind::Up(crossterm::event::MouseButton::Left) => {
                        app.finish_selection();
                        // Only copy if there's a real selection (not just a click)
                        if app.selection_range().is_some() {
                            let is_streaming = matches!(
                                app.phase,
                                AgentPhase::WaitingForLlm | AgentPhase::ExecutingTools
                            );
                            let chat = ChatPanel {
                                messages: &app.chat_messages,
                                streaming_text: &app.streaming_text,
                                is_streaming,
                                spinner_frame: app.spinner_frame,
                                scroll_offset: app.scroll_offset,
                                selection: None,
                            };
                            let plain = chat.build_plain_lines(app.chat_area.width);
                            app.copy_selection_to_clipboard(&plain);
                            app.status_message = Some("Copied!".into());
                        }
                    }
                    _ => {}
                }
            }

            AppEvent::Tick => {
                app.on_tick();
            }

            AppEvent::StreamChunk(text) => {
                app.on_stream_chunk(text);
            }

            AppEvent::StreamDone {
                content,
                prompt_tokens,
                completion_tokens,
            } => {
                app.on_stream_done(content, prompt_tokens, completion_tokens);
            }

            AppEvent::StreamToolCalls {
                calls,
                text,
                prompt_tokens,
                completion_tokens,
            } => {
                app.on_stream_tool_calls(calls, text, prompt_tokens, completion_tokens);
            }

            AppEvent::StreamError(err) => {
                app.on_stream_error(err);
            }

            AppEvent::ToolResult { name, output } => {
                app.on_tool_result(name, output);
            }

            AppEvent::Resize(_, _) => {
                // ratatui handles resize automatically on next draw()
            }
        }
    }

    // Restore terminal
    terminal::disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    // Auto-save session
    let has_user_msgs = app.history.iter().any(|m| m.role == "user");
    if has_user_msgs {
        if let Ok(path) = crate::session::save(
            &app.history,
            &app.client.model,
            &app.workspace_root,
            app.session_id.as_deref(),
        ) {
            let id = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
            println!("Session auto-saved (id: {id})");
        }
    }

    Ok(())
}
