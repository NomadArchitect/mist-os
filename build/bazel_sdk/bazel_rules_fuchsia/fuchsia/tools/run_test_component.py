#!/usr/bin/env python3
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import subprocess
from typing import List, Tuple

from fuchsia_task_lib import *


class FuchsiaTaskRunTestComponent(FuchsiaTask):
    def parse_known_args(
        self, parser: ScopedArgumentParser
    ) -> Tuple[argparse.Namespace, List[str]]:
        """Parses arguments."""

        parser.add_argument(
            "--ffx",
            type=parser.path_arg(),
            help="A path to the ffx tool.",
            required=True,
        )
        parser.add_argument(
            "--url",
            type=str,
            help="The full component url.",
            required=True,
        )
        parser.add_argument(
            "--package-manifest",
            type=parser.path_arg(),
            help="A path to the package manifest json file.",
        )
        parser.add_argument(
            "--target",
            help="Optionally specify the target fuchsia device.",
            required=False,
            scope=ArgumentScope.GLOBAL,
        )
        parser.add_argument(
            "--realm",
            help="Optionally specify the target realm to run this test.",
            required=False,
            scope=ArgumentScope.GLOBAL,
        )
        return parser.parse_known_args()

    def run(self, parser: ScopedArgumentParser) -> None:
        args, remainder_args = self.parse_known_args(parser)
        ffx = [args.ffx] + (["--target", args.target] if args.target else [])
        url = (
            args.url.replace(
                "{{PACKAGE_NAME}}",
                json.loads(args.package_manifest.read_text())["package"][
                    "name"
                ],
            )
            if args.package_manifest
            else args.url
        )

        try:
            print(
                Terminal.info(
                    f"Forwarding unrecognized args to test: {remainder_args}"
                )
            )
            subprocess.check_call(
                [
                    *ffx,
                    "test",
                    "run",
                    *(["--realm", args.realm] if args.realm else []),
                    url,
                    "--",
                    *remainder_args,
                ]
            )
        except subprocess.CalledProcessError as e:
            if e.returncode != 1:
                raise e
            raise TaskExecutionException(
                f"Test Failures!", is_caught_failure=True
            )


if __name__ == "__main__":
    FuchsiaTaskRunTestComponent.main()
