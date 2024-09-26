// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package result

import (
	"encoding/json"
	"fmt"
	"io/fs"
	"path/filepath"
	"strings"
	"text/template"
)

var (
	AllTemplates map[string]*template.Template
)

func init() {
	AllTemplates = make(map[string]*template.Template)
}

func Initialize(c *ResultConfig) error {
	// Save the config file to the out directory (if defined).
	if b, err := json.MarshalIndent(c, "", "  "); err != nil {
		return err
	} else {
		plusFile("_config.json", b)
	}

	Config = c

	// Ensure no allowlist entries end in a "/"
	for _, check := range c.Checks {
		for k := range check.Allowlist {
			if strings.HasSuffix(k, "/") {
				return fmt.Errorf("\nAllowlist \"%s\" has an entry \"%s\" that ends with \"/\". This is not allowed.\nPlease remove the trailing slash in this allowlist entry.", check.Name, k)
			}
		}
	}

	return initializeTemplates()
}

func initializeTemplates() error {
	for _, templateCategory := range Config.Templates {
		for _, templatePath := range templateCategory.Paths {
			templatePath = filepath.Join(Config.FuchsiaDir, templatePath)
			if err := filepath.WalkDir(templatePath, func(currentPath string, info fs.DirEntry, err error) error {
				if err != nil {
					return err
				}

				if !info.IsDir() {
					if temp, err := template.New(filepath.Base(currentPath)).ParseFiles(currentPath); err != nil {
						return err
					} else {
						relPath, err := filepath.Rel(templatePath, currentPath)
						if err != nil {
							return err
						}
						plusVal(NumInitTemplates, currentPath)
						AllTemplates[relPath] = temp
					}
				}
				return nil
			}); err != nil {
				return err
			}
		}
	}
	return nil
}
