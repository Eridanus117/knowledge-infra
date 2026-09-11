/// CLI telemetry is intentionally silent: diagnostics belong only in the JSON envelope.
pub struct Telemetry;
#[allow(dead_code)]
impl Telemetry {
    #[must_use]
    pub const fn disabled() -> Self {
        Self
    }

    pub fn event(&self, _name: &str) {}
}
