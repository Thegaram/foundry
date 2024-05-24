use alloy_json_abi::JsonAbi;
use alloy_sol_macro_expander::expand::expand;
use alloy_sol_macro_input::{tokens_for_sol, SolInput, SolInputKind};
use eyre::Result;
use foundry_common::fs;
use proc_macro2::{Ident, Span, TokenStream};
use serde_json::Value;
use std::path::{Path, PathBuf};
pub struct SolMacroGen {
    pub path: PathBuf,
    pub name: String,
    pub expansion: Option<TokenStream>,
}

impl SolMacroGen {
    pub fn new(path: PathBuf, name: String) -> Self {
        Self { path, name, expansion: None }
    }

    pub fn get_json_abi(&self) -> Result<(JsonAbi, Option<String>)> {
        let json = std::fs::read(&self.path)?;

        // Need to do this to get the abi in the next step.
        let json: Value = serde_json::from_slice(&json)?;

        // Get the abi from the json.
        let json_abi = if let Some(abi) = json.get("abi") {
            let json: JsonAbi = serde_json::from_str(&abi.clone().to_string())?;
            json
        } else {
            return Err(eyre::eyre!("No ABI found in JSON file"));
        };

        let bytecode = if let Some(bytecode) = json.get("bytecode") {
            Some(bytecode.to_string())
        } else {
            None
        };

        Ok((json_abi, bytecode))
    }
}

pub struct MultiSolMacroGen {
    pub artifacts_path: PathBuf,
    pub instances: Vec<SolMacroGen>,
}

impl MultiSolMacroGen {
    pub fn new(artifacts_path: &Path, instances: Vec<SolMacroGen>) -> Self {
        Self { artifacts_path: artifacts_path.to_path_buf(), instances }
    }

    fn generate_bindings(&mut self) -> Result<()> {
        for instance in &mut self.instances {
            let (mut json_abi, _maybe_bytecode) = instance.get_json_abi()?;

            json_abi.dedup();
            let sol_str = json_abi.to_sol(&instance.name, None);

            let ident_name: Ident = Ident::new(&instance.name, Span::call_site());

            let tokens = tokens_for_sol(&ident_name, &sol_str)
                .map_err(|e| eyre::eyre!("Failed to get sol tokens: {e}"))?;

            let tokens = quote::quote! {
                #[derive(Debug)]
                #[sol(rpc)]
                #tokens
            };

            let input: SolInput =
                syn::parse2(tokens).map_err(|e| eyre::eyre!("Failed to parse SolInput: {e}"))?;

            let SolInput { attrs: _attrs, path: _path, kind } = input;

            let tokens = match kind {
                SolInputKind::Sol(file) => {
                    // TODO: Add attributes if needed to file.attrs
                    expand(file).map_err(|e| eyre::eyre!("Failed to expand SolInput: {e}"))?
                }
                _ => unreachable!(),
            };

            instance.expansion = Some(tokens);
        }

        Ok(())
    }

    pub fn write_to_crate(
        &mut self,
        name: &str,
        version: &str,
        bindings_path: &Path,
        single_file: bool,
    ) -> Result<()> {
        self.generate_bindings()?;

        let src = bindings_path.join("src");

        let _ = fs::create_dir_all(&src);

        // Write Cargo.toml
        let cargo_toml_path = bindings_path.join("Cargo.toml");
        let toml_contents = format!(
            r#"
[package]
name = "{}"
version = "{}"
edition = "2021"

[dependencies]
alloy-sol-types = "0.7.4"
alloy-contract = {{ git = "https://github.com/alloy-rs/alloy", rev = "64feb9b" }}"#,
            name, version
        );

        fs::write(cargo_toml_path, toml_contents)
            .map_err(|e| eyre::eyre!("Failed to write Cargo.toml: {e}"))?;

        // Write src
        let mut lib_contents = if single_file {
            String::from("#![allow(unused_imports, clippy::all)]\n\n//! This module contains the sol! generated bindings for solidity contracts.\n//! This is autogenerated code.\n//! Do not manually edit these files.\n//! These files may be overwritten by the codegen system at any time.\n")
        } else {
            String::from("#![allow(unused_imports)]\n")
        };

        for instance in &self.instances {
            let name = instance.name.to_lowercase();
            let contents = instance.expansion.as_ref().unwrap().to_string();

            if !single_file {
                let path = src.join(format!("{}.rs", name));
                fs::write(path, contents).map_err(|e| eyre::eyre!("Failed to write file: {e}"))?;
                lib_contents += &format!("pub mod {};\n", name);
            } else {
                lib_contents += &contents;
            }
        }

        if !single_file {
            lib_contents += "\nextern crate alloy_sol_types;\nextern crate core;\n";
        }

        let lib_path = src.join("lib.rs");
        fs::write(lib_path, lib_contents)
            .map_err(|e| eyre::eyre!("Failed to write lib.rs: {e}"))?;

        Ok(())
    }

    pub fn write_to_module(&mut self, bindings_path: &Path, single_file: bool) -> Result<()> {
        self.generate_bindings()?;

        let _ = fs::create_dir_all(bindings_path);

        let mut mod_contents = String::from("#![allow(clippy::all)]\n//! This module contains the sol! generated bindings for solidity contracts.\n//! This is autogenerated code.\n//! Do not manually edit these files.\n//! These files may be overwritten by the codegen system at any time.\n");
        for instance in &self.instances {
            let name = instance.name.to_lowercase();
            if !single_file {
                mod_contents += &format!("pub mod {};\n", instance.name.to_lowercase());
                let contents = format!(
                    "//! This module was autogenerated by the alloy sol!.\n//! More information can be found here <https://docs.rs/alloy-sol-macro/latest/alloy_sol_macro/macro.sol.html>.\n",
                ) + &instance.expansion.as_ref().unwrap().to_string();
                fs::write(bindings_path.join(format!("{}.rs", name)), contents)
                    .map_err(|e| eyre::eyre!("Failed to write file: {e}"))?;
            } else {
                let contents = format!(
                    "pub use {}::*;\n//! This module was autogenerated by the alloy sol!.\n//! More information can be found here <https://docs.rs/alloy-sol-macro/latest/alloy_sol_macro/macro.sol.html>.\n",
                    name
                ) + &instance.expansion.as_ref().unwrap().to_string() + "\n\n";
                mod_contents += &contents;
            }
        }

        let mod_path = bindings_path.join("mod.rs");
        fs::write(mod_path, mod_contents)
            .map_err(|e| eyre::eyre!("Failed to write mod.rs: {e}"))?;

        Ok(())
    }
}
