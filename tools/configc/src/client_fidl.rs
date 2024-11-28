// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common::load_manifest;
use anyhow::{format_err, Context as _, Error};
use argh::FromArgs;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command as Process, Stdio};

#[derive(FromArgs, PartialEq, Debug)]
/// Generates a FIDL client library from a given manifest.
#[argh(subcommand, name = "fidl")]
pub struct GenerateFidlSource {
    /// compiled manifest containing the config declaration
    #[argh(option)]
    cm: PathBuf,

    /// path to which to output FIDL source file
    #[argh(option)]
    output: PathBuf,

    /// name for the internal FIDL library
    #[argh(option)]
    library_name: String,

    /// path to fidl-format binary
    #[argh(option)]
    fidl_format: PathBuf,
}

impl GenerateFidlSource {
    pub fn generate(self) -> Result<(), Error> {
        let component = load_manifest(&self.cm).context("loading component manifest")?;
        let config_decl = component
            .config
            .as_ref()
            .ok_or_else(|| anyhow::format_err!("missing config declaration in manifest"))?;

        let fidl_contents = config_client::fidl::create_fidl_source(config_decl, self.library_name);

        let formatted_fidl_contents = format_source(self.fidl_format, fidl_contents)?;

        let mut fidl_out_file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(self.output)
            .context("opening output file")?;
        fidl_out_file
            .write(formatted_fidl_contents.as_bytes())
            .context("writing FIDL file to output")?;

        Ok(())
    }
}

fn format_source(fidl_format: PathBuf, contents: String) -> Result<String, Error> {
    let mut process = Process::new(fidl_format)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("could not spawn fidl-format process")?;
    process
        .stdin
        .as_mut()
        .ok_or_else(|| format_err!("could not get stdin for fidl-format process"))?
        .write_all(contents.as_bytes())
        .context("could not write unformatted source to stdin of fidl-format")?;
    let output =
        process.wait_with_output().context("could not wait for fidl-format process to exit")?;

    if !output.status.success() {
        return Err(format_err!("failed to format FIDL source: {:#?}", output));
    }

    let output = String::from_utf8(output.stdout)
        .context("output from fidl-format is not UTF-8 compatible")?;
    Ok(output)
}
