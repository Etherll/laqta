use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

/// stderr plus a daily-rolling file at `<data_local_dir>/Laqta/logs/`, and a panic hook that logs
/// the message and backtrace. Keep the returned guard alive to flush the file writer.
pub fn init() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let dir = dirs::data_local_dir().map(|d| d.join("Laqta").join("logs"));
    let (file_layer, guard) = match dir.as_ref().map(|d| (d, std::fs::create_dir_all(d))) {
        Some((d, Ok(()))) => {
            let (writer, guard) = tracing_appender::non_blocking(tracing_appender::rolling::daily(d, "laqta.log"));
            (Some(fmt::layer().with_ansi(false).with_writer(writer)), Some(guard))
        }
        _ => (None, None),
    };
    let _ = tracing_subscriber::registry().with(filter).with(fmt::layer().with_writer(std::io::stderr)).with(file_layer).try_init();
    std::panic::set_hook(Box::new(|info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!("panic: {info}\n{backtrace}");
    }));
    guard
}
