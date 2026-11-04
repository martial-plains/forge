// SPDX-License-Identifier: MIT OR Apache-2.0
//! Plugin metadata. The API plugins call is defined in WIT
//! (`wit/forge-editor.wit`), not here.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub author: Option<String>,
    pub provides_major_modes: Vec<u32>,
    pub provides_minor_modes: Vec<u32>,
    pub provides_commands: Vec<String>,
    pub permissions: Vec<Permission>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Permission {
    FileRead,
    FileWrite,
    Network,
    Process,
    Clipboard,
}
