//! Small desktop-notification abstraction used for transfer limits and files.

use std::sync::Arc;

pub trait Notifier: Send + Sync {
    fn notify(&self, summary: &str, body: &str);
}

pub struct NoopNotifier;

impl Notifier for NoopNotifier {
    fn notify(&self, _summary: &str, _body: &str) {}
}

#[cfg(target_os = "linux")]
pub struct DesktopNotifier;

#[cfg(target_os = "linux")]
impl Notifier for DesktopNotifier {
    fn notify(&self, summary: &str, body: &str) {
        use std::sync::OnceLock;
        use zbus::blocking::{Connection, Proxy};
        static CONNECTION: OnceLock<Option<Connection>> = OnceLock::new();
        let Some(connection) = CONNECTION
            .get_or_init(|| Connection::session().ok())
            .as_ref()
        else {
            return;
        };
        let Ok(proxy) = Proxy::new(
            connection,
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
        ) else {
            return;
        };
        let hints = std::collections::HashMap::<String, zbus::zvariant::Value<'static>>::new();
        let _ = proxy.call::<_, _, u32>(
            "Notify",
            &(
                "Sean Toolbox",
                0u32,
                "sean-toolbox",
                summary,
                body,
                Vec::<&str>::new(),
                hints,
                5000i32,
            ),
        );
    }
}

#[cfg(windows)]
pub struct DesktopNotifier;

#[cfg(windows)]
impl Notifier for DesktopNotifier {
    fn notify(&self, summary: &str, body: &str) {
        let _ = notify_rust::Notification::new()
            .summary(summary)
            .body(body)
            .show();
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
pub struct DesktopNotifier;

#[cfg(not(any(target_os = "linux", windows)))]
impl Notifier for DesktopNotifier {
    fn notify(&self, summary: &str, body: &str) {
        eprintln!("Sean Toolbox notification: {summary}: {body}");
    }
}

pub fn desktop_notifier() -> Arc<dyn Notifier> {
    Arc::new(DesktopNotifier)
}
