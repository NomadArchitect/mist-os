// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Typesafe wrappers around verifying the board file.

use fidl_fuchsia_io as fio;
use thiserror::Error;
use zx_status::Status;

/// An error encountered while verifying the board.
#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum VerifyBoardError {
    #[error("while opening the file: {0}")]
    OpenFile(#[from] fuchsia_fs::node::OpenError),

    #[error("while reading the file: {0}")]
    ReadFile(#[from] fuchsia_fs::file::ReadError),

    #[error("expected board name {} found {}", expected, found)]
    VerifyContents { expected: String, found: String },
}

pub(crate) async fn verify_board(
    proxy: &fio::DirectoryProxy,
    expected_contents: &str,
) -> Result<(), VerifyBoardError> {
    let file = match fuchsia_fs::directory::open_file(proxy, "board", fio::PERM_READABLE).await {
        Ok(file) => Ok(file),
        Err(fuchsia_fs::node::OpenError::OpenError(Status::NOT_FOUND)) => return Ok(()),
        Err(e) => Err(e),
    }
    .map_err(VerifyBoardError::OpenFile)?;

    let contents =
        fuchsia_fs::file::read_to_string(&file).await.map_err(VerifyBoardError::ReadFile)?;

    if expected_contents != contents {
        return Err(VerifyBoardError::VerifyContents {
            expected: expected_contents.to_string(),
            found: contents,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestUpdatePackage;
    use assert_matches::assert_matches;
    use fuchsia_async as fasync;

    #[fasync::run_singlethreaded(test)]
    async fn verify_board_success_file_exists() {
        let p = TestUpdatePackage::new().add_file("board", "kourtney").await;
        assert_matches!(p.verify_board("kourtney").await, Ok(()));
    }

    #[fasync::run_singlethreaded(test)]
    async fn verify_board_success_file_does_not_exist() {
        let p = TestUpdatePackage::new();
        assert_matches!(p.verify_board("kim").await, Ok(()));
    }

    #[fasync::run_singlethreaded(test)]
    async fn verify_board_failure_verify_contents() {
        let p = TestUpdatePackage::new().add_file("board", "khloe").await;
        assert_matches!(
            p.verify_board("kendall").await,
            Err(VerifyBoardError::VerifyContents { expected, found })
                if expected=="kendall" && found=="khloe"
        );
    }
}
