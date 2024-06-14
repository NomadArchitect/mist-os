// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Package utils provides shared utilities needed within orchestrate.
package orchestrate

import (
	"context"
	"fmt"
	"strings"
	"time"
)

// RunWithRetries invokes `callable` retrying up to `maxRetries` and delaying by
// `retryDelay` each time there's an error.
func RunWithRetries(ctx context.Context, retryDelay time.Duration, maxRetries int, callable func() error) error {
	currAttempt := 0
	for {
		if err := callable(); err == nil || currAttempt == maxRetries {
			return err
		}
		currAttempt++

		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(retryDelay):
		}
	}
}

// Appends a file path to the PATH variable inside an environ. If PATH isn't in the environment it
// will be created. This also assumes that there is at most one PATH in the environment.
func AppendPath(environ []string, dirs ...string) []string {
	var result []string
	pathFound := false
	for _, e := range environ {
		if strings.HasPrefix(e, "PATH=") {
			pathFound = true
			result = append(result, fmt.Sprintf("%s:%s", e, strings.Join(dirs, ":")))
		} else {
			result = append(result, e)
		}
	}
	if !pathFound && len(dirs) > 0 {
		result = append(result, fmt.Sprintf("PATH=%s", strings.Join(dirs, ":")))
	}
	return result
}
