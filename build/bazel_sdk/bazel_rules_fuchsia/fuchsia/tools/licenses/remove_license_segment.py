#!/usr/bin/env python3

# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import re
import sys


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--license_id", required=True)
    parser.add_argument("--cut_after", required=True)
    parser.add_argument("--exit_on_failure", default=False)
    args = parser.parse_args()

    with open(args.input, "r") as spdx:
        data = json.load(spdx)

    found_segment = False
    error_msg = "Did not find licenseID {} in spdx file {}.".format(
        args.license_id, args.input
    )
    for d in data["hasExtractedLicensingInfos"]:
        if d["licenseId"] == args.license_id:
            error_msg = (
                "Did not find string pattern {} in license text.".format(
                    args.cut_after
                )
            )
            text = d["extractedText"]
            match = re.search(args.cut_after, text)
            if match:
                d["extractedText"] = text[: match.end()]
                found_segment = True
            break

    if not found_segment:
        if args.exit_on_failure:
            raise ValueError(error_msg)
        else:
            print(error_msg)

    with open(args.output, "w") as spdx:
        json.dump(data, spdx, ensure_ascii=False, indent="    ")


if __name__ == "__main__":
    sys.exit(main())
