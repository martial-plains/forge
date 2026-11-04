// SPDX-License-Identifier: MIT OR Apache-2.0
//! Plugin metadata and management.

use crate::plugin::api::PluginManifest;
use anyhow::{Result, bail};
use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
pub struct PluginRegistry {
    plugins: HashMap<String, PluginManifest>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_plugin(&mut self, manifest: PluginManifest) -> Result<()> {
        if self.plugins.contains_key(&manifest.id) {
            bail!("Plugin already registered: {}", manifest.id);
        }
        self.plugins.insert(manifest.id.clone(), manifest);
        Ok(())
    }

    pub fn get_plugin(&self, id: &str) -> Option<&PluginManifest> {
        self.plugins.get(id)
    }

    pub fn all_plugins(&self) -> impl Iterator<Item = &PluginManifest> {
        self.plugins.values()
    }
}
