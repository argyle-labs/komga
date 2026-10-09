//! Dynamic (subprocess) entrypoint for the komga plugin.
//!
//! The [`Plugin`](plugin_toolkit::plugin::Plugin) builder registers the
//! [`ServiceBackend`] and the `komga.` tools and emits all the wire dispatch,
//! so the plugin hand-writes no op strings and owns no runtime — it reaches
//! orca only through the socket.
plugin_toolkit::instrument::bootstrap!();

use komga::KomgaBackend;
use plugin_toolkit::plugin::Plugin;

// The builder does not force-link the lib, and without this the linker drops
// every `#[orca_tool]` / `#[endpoint_resource]` registration.
#[allow(unused_imports)]
use komga::tools as _;

fn main() -> plugin_toolkit::anyhow::Result<()> {
    Plugin::named("komga")
        .version(env!("CARGO_PKG_VERSION"))
        .service(KomgaBackend::new("komga"))
        .tools(["komga."])
        .serve()
}
