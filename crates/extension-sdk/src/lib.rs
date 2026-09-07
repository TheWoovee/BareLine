// SPDX-License-Identifier: MIT OR Apache-2.0
//! Stable author-facing protocol, manifest builder and bounded transport harness.
pub use bareline_extensions_protocol::*;
pub const WIT_WORLD: &str = include_str!("../wit/extension.wit");
pub struct ManifestBuilder {
    manifest: ExtensionManifest,
}
impl ManifestBuilder {
    pub fn new(
        id: &str,
        version: &str,
        publisher: &str,
        entry: &str,
    ) -> Result<Self, &'static str> {
        if !valid_id(id) || version.is_empty() || publisher.is_empty() || entry.is_empty() {
            return Err("invalid extension identity");
        }
        Ok(Self {
            manifest: ExtensionManifest {
                schema_version: 1,
                id: id.into(),
                version: version.into(),
                publisher: publisher.into(),
                minimum_protocol: PROTOCOL_VERSION,
                maximum_protocol: PROTOCOL_VERSION,
                entry_component: entry.into(),
                commands: vec![],
                background_commands: vec![],
                panels: vec![],
                capabilities: vec![],
            },
        })
    }
    pub fn capability(mut self, capability: Capability) -> Self {
        if !self.manifest.capabilities.contains(&capability) {
            self.manifest.capabilities.push(capability);
        }
        self
    }
    pub fn command(mut self, id: &str) -> Result<Self, &'static str> {
        if !valid_id(id) {
            return Err("invalid command ID");
        }
        self.manifest.commands.push(id.into());
        Ok(self)
    }
    pub fn panel(mut self, id: &str) -> Result<Self, &'static str> {
        if !valid_id(id) {
            return Err("invalid panel ID");
        }
        self.manifest.panels.push(id.into());
        Ok(self)
    }
    pub fn build(self) -> ExtensionManifest {
        self.manifest
    }
}
/// Test the exact framing boundary without native handles or privileged imports.
pub fn roundtrip(message: &Envelope) -> Result<Envelope, ProtocolError> {
    let mut bytes = Vec::new();
    write_frame(&mut bytes, message)?;
    read_frame(&mut &bytes[..])
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manifest_has_no_implicit_grants() {
        let manifest = ManifestBuilder::new("org.bareline.fixture", "1", "fixture", "fixture.wasm")
            .unwrap()
            .command("fixture.run")
            .unwrap()
            .build();
        assert!(manifest.capabilities.is_empty());
        assert_eq!(manifest.minimum_protocol, 1);
    }
}

#[cfg(target_arch = "wasm32")]
pub mod bindings {
    wit_bindgen::generate!({ path: "wit", world: "extension", pub_export_macro: true });
}
