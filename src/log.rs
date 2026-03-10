use std::{
    io::{BufRead, Write},
    sync::{Arc, Mutex},
};

use bevy::{
    app::App,
    log::tracing_subscriber::{self, EnvFilter, Layer},
    prelude::{MessageWriter, ResMut, Resource},
};

use tracing_subscriber::registry::Registry;
use crate::{PrintConsoleLine};

/// Buffers logs written by bevy at runtime
#[derive(Resource)]
pub struct BevyLogBuffer(Arc<Mutex<std::io::Cursor<Vec<u8>>>>);

impl BevyLogBuffer {
    /// Returns a reference to the internal log buffer.
    pub fn buffer(&self) -> &Arc<Mutex<std::io::Cursor<Vec<u8>>>> {
        &self.0
    }
}

/// Writer implementation which writes into a buffer resource inside the bevy world
pub struct BevyLogBufferWriter(Arc<Mutex<std::io::Cursor<Vec<u8>>>>);

impl Write for BevyLogBufferWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // let lock = self.0.upgrade().unwrap();
        let mut lock = self
            .0
            .lock()
            .map_err(|e| std::io::Error::other(format!("Failed to lock buffer: {}", e)))?;
        lock.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // let lock = self.0.upgrade().unwrap();
        let mut lock = self
            .0
            .lock()
            .map_err(|e| std::io::Error::other(format!("Failed to lock buffer: {}", e)))?;
        lock.flush()
    }
}

/// Flushes the log buffer and sends its content to the console
// Unused import removed
/// Flushes the log buffer and sends its content to the console
pub fn send_log_buffer_to_console(
    buffer: ResMut<BevyLogBuffer>,
    mut console_lines: MessageWriter<PrintConsoleLine>,
    // mut command_writer: EventWriter<ConsoleCommandEntered>,
) {
    let mut buffer = buffer.0.lock().unwrap();
    // read and clean buffer
    let buffer = buffer.get_mut();
    for line in buffer.lines().map_while(Result::ok) {
        // Dispatch console command if log line starts with '> '
        if let Some(cmd) = line.strip_prefix("> ") {
            // Split command and args
            let mut parts = cmd.split_whitespace();
            if let Some(_) = parts.next() {
                let _: Vec<String> = parts.map(|s| s.to_string()).collect();
                // Command dispatch removed; handle in plugin system.
            }
        }
        console_lines.write(PrintConsoleLine { line });
    }
    buffer.clear();
}

/// Creates a tracing layer which writes logs into a buffer resource inside the bevy world
/// This is used by the console plugin to capture logs written by bevy
/// Use [make_filtered_layer] for more customization options.
pub fn make_layer(
    app: &mut App,
) -> Option<Box<dyn tracing_subscriber::Layer<Registry> + Send + Sync>> {
    setup_layer(app, None)
}

/// Creates a tracing layer which writes logs into a buffer resource inside the bevy world
/// Uses a custom [EnvFilter] string, allowing for a different subset of log entries to be
/// captured by the console.
/// This is used by the console plugin to capture logs written by bevy
///
/// ## Example
/// ```ignore
/// DefaultPlugins.set(LogPlugin {
///    filter: log::DEFAULT_FILTER.to_string(),
///    level: log::Level::INFO,
///    custom_layer: |app: &mut App| make_filtered_layer(app,
///        "mygame=info,warn,debug,error".to_string())
///})
///```
pub fn make_filtered_layer(
    app: &mut App,
    filter: String,
) -> Option<Box<dyn tracing_subscriber::Layer<Registry> + Send + Sync>> {
    let env_filter = EnvFilter::builder().parse_lossy(filter);
    setup_layer(app, Some(env_filter))
}

/// Performs common layer setup
fn setup_layer(
    app: &mut App,
    filter: Option<EnvFilter>,
) -> Option<Box<dyn tracing_subscriber::Layer<Registry> + Send + Sync>> {
    let buffer = Arc::new(Mutex::new(std::io::Cursor::new(Vec::new())));
    app.insert_resource(BevyLogBuffer(buffer.clone()));
    // Removed system registration for send_log_buffer_to_console. Call directly from plugin system.

    let layer: Box<dyn tracing_subscriber::Layer<Registry> + Send + Sync> = match filter {
        Some(filter) => Box::new(
            tracing_subscriber::fmt::Layer::new()
                .with_target(false)
                .with_ansi(true)
                .with_writer(move || BevyLogBufferWriter(buffer.clone()))
                .with_filter(filter),
        ),
        None => Box::new(
            tracing_subscriber::fmt::Layer::new()
                .with_target(false)
                .with_ansi(true)
                .with_writer(move || BevyLogBufferWriter(buffer.clone())),
        ),
    };

    Some(layer)
}
