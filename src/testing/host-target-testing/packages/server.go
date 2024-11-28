// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package packages

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io/fs"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	tuf_data "github.com/theupdateframework/go-tuf/data"

	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/build"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

type Server struct {
	Dir          string
	BlobStore    BlobStore
	URL          string
	Hash         string
	server       *http.Server
	shuttingDown chan struct{}
}

type httpBlobStore struct {
	ctx       context.Context
	blobStore BlobStore
}

func (f httpBlobStore) Open(path string) (fs.File, error) {
	parts := strings.Split(path, "/")

	switch len(parts) {
	case 2:
		if parts[0] != "blobs" {
			return nil, os.ErrNotExist
		}

		merkle, err := build.DecodeMerkleRoot([]byte(parts[1]))
		if err != nil {
			return nil, os.ErrNotExist
		}

		return f.blobStore.OpenBlob(f.ctx, nil, merkle)
	case 3:
		if parts[0] != "blobs" {
			return nil, os.ErrNotExist
		}

		var deliveryBlobType *int
		if d, err := strconv.Atoi(parts[1]); err == nil {
			deliveryBlobType = &d
		} else {
			return nil, os.ErrNotExist
		}

		merkle, err := build.DecodeMerkleRoot([]byte(parts[2]))
		if err != nil {
			return nil, os.ErrNotExist
		}

		return f.blobStore.OpenBlob(f.ctx, deliveryBlobType, merkle)
	default:
		return nil, os.ErrNotExist
	}
}

func newServer(
	ctx context.Context,
	dir string,
	blobStore BlobStore,
	localHostname string,
	repoName string,
	repoPort int,
) (*Server, error) {
	listener, err := net.Listen("tcp", ":"+strconv.Itoa(repoPort))
	if err != nil {
		return nil, err
	}

	port := listener.Addr().(*net.TCPAddr).Port
	logger.Infof(ctx, "Serving %s on :%d", dir, port)

	configURL, configHash, config, err := genConfig(dir, localHostname, repoName, port)
	if err != nil {
		listener.Close()
		return nil, err
	}
	logger.Infof(ctx, "%s [repo serve] config.json: %s\n",
		time.Now().Format("2006-01-02 15:04:05"), string(config))

	mux := http.NewServeMux()
	mux.HandleFunc(fmt.Sprintf("/%s/config.json", repoName), func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(200)
		w.Write(config)
	})
	// Blobs requests come as `/blobs/<merkle>` so the directory we actually
	// serve from should be the parent directory of the blobsDir and the blobsDir
	// should be called `blobs`.
	mux.Handle("/blobs/", http.FileServer(http.FS(httpBlobStore{ctx, blobStore})))
	mux.Handle("/", http.FileServer(http.Dir(dir)))

	server := &http.Server{
		Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			lw := &loggingWriter{w, 0}
			mux.ServeHTTP(lw, r)
			logger.Infof(ctx, "%s [repo serve] %d %s\n",
				time.Now().Format("2006-01-02 15:04:05"), lw.status, r.RequestURI)
		}),
	}

	go func() {
		if err := server.Serve(listener); err != http.ErrServerClosed {
			logger.Fatalf(ctx, "failed to shutdown server: %s", err)
		}
	}()

	shuttingDown := make(chan struct{})

	// Tear down the server if the context expires.
	go func() {
		select {
		case <-shuttingDown:
		case <-ctx.Done():
			server.Shutdown(ctx)
		}
	}()

	return &Server{
		Dir:          dir,
		BlobStore:    blobStore,
		URL:          configURL,
		Hash:         configHash,
		server:       server,
		shuttingDown: shuttingDown,
	}, err
}

func (s *Server) Shutdown(ctx context.Context) {
	s.server.Shutdown(ctx)
	close(s.shuttingDown)
}

type loggingWriter struct {
	http.ResponseWriter
	status int
}

func (lw *loggingWriter) WriteHeader(status int) {
	lw.status = status
	lw.ResponseWriter.WriteHeader(status)
}

// writeConfig writes the source config to the repository.
func genConfig(dir string, localHostname string, repoName string, port int) (configURL string, configHash string, config []byte, err error) {
	type repositoryStorageType string

	type keyConfig struct {
		Type  string `json:"type"`
		Value string `json:"value"`
	}

	getRootKeys := func(root *tuf_data.Root) ([]keyConfig, error) {
		rootKeys := make(map[string]struct{})
		if role, ok := root.Roles["root"]; ok {
			for _, id := range role.KeyIDs {
				if key, ok := root.Keys[id]; ok {
					switch key.Type {
					case tuf_data.KeyTypeEd25519:
						var kv struct {
							Public tuf_data.HexBytes `json:"public"`
						}
						if err := json.Unmarshal(key.Value, &kv); err != nil {
							return nil, fmt.Errorf("failed to unmarshal key: %w", err)
						}
						rootKeys[kv.Public.String()] = struct{}{}
					default:
						return nil, fmt.Errorf("unexpected key type: %q", key.Type)
					}
				}
			}
		}
		rootKeyConfigs := make([]keyConfig, 0, len(rootKeys))
		for key := range rootKeys {
			rootKeyConfigs = append(rootKeyConfigs, keyConfig{"ed25519", key})
		}
		return rootKeyConfigs, nil
	}

	type mirrorConfig struct {
		MirrorURL string `json:"mirror_url"`
		Subscribe bool   `json:"subscribe"`
		BlobURL   string `json:"blob_mirror_url,omitempty"`
	}

	type repositoryConfig struct {
		RepoURL          string                `json:"repo_url"`
		RootKeys         []keyConfig           `json:"root_keys"`
		Mirrors          []mirrorConfig        `json:"mirrors"`
		RootVersion      uint32                `json:"root_version"`
		RootThreshold    uint32                `json:"root_threshold"`
		UpdatePackageURL string                `json:"update_package_url,omitempty"`
		UseLocalMirror   bool                  `json:"use_local_mirror,omitempty"`
		StorageType      repositoryStorageType `json:"storage_type,omitempty"`
	}

	f, err := os.Open(filepath.Join(dir, "root.json"))
	if err != nil {
		return "", "", nil, err
	}
	defer f.Close()

	var signed tuf_data.Signed
	if err := json.NewDecoder(f).Decode(&signed); err != nil {
		return "", "", nil, err
	}

	var root tuf_data.Root
	if err := json.Unmarshal(signed.Signed, &root); err != nil {
		return "", "", nil, err
	}

	rootKeys, err := getRootKeys(&root)
	if err != nil {
		return "", "", nil, err
	}

	hostname := strings.ReplaceAll(localHostname, "%", "%25")

	var mirrorURL string
	if strings.Contains(hostname, ":") {
		// This is an IPv6 address, use brackets for an IPv6 literal
		mirrorURL = fmt.Sprintf("http://[%s]:%d", hostname, port)
	} else {
		mirrorURL = fmt.Sprintf("http://%s:%d", hostname, port)
	}

	mirror := []mirrorConfig{
		{
			MirrorURL: mirrorURL,
			Subscribe: true,
			BlobURL:   fmt.Sprintf("%s/blobs", mirrorURL),
		},
	}

	configURL = fmt.Sprintf("%s/%s/config.json", mirrorURL, repoName)

	var rootThreshold int
	if rootRole, ok := root.Roles["root"]; ok {
		rootThreshold = rootRole.Threshold
	}

	config, err = json.Marshal(&repositoryConfig{
		RepoURL:        fmt.Sprintf("fuchsia-pkg://%s", repoName),
		RootKeys:       rootKeys,
		Mirrors:        mirror,
		RootVersion:    uint32(root.Version),
		RootThreshold:  uint32(rootThreshold),
		UseLocalMirror: false,
		StorageType:    "ephemeral",
	})
	if err != nil {
		return "", "", nil, err
	}
	h := sha256.Sum256(config)
	configHash = hex.EncodeToString(h[:])

	return configURL, configHash, config, nil
}
