// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_TESTS_INTEGRATION_INPUT_TESTS_WEB_TEST_BASE_WEB_APP_BASE_H_
#define SRC_UI_TESTS_INTEGRATION_INPUT_TESTS_WEB_TEST_BASE_WEB_APP_BASE_H_

#include <fidl/fuchsia.element/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <fidl/fuchsia.web/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fidl/cpp/channel.h>

#include <string>

#include <src/lib/json_parser/json_parser.h>

namespace integration_tests {

// Parse buffer to string.
std::string StringFromBuffer(const fuchsia_mem::Buffer& buffer);

// Parse buffer to json object. Exception when parsing failed.
rapidjson::Document JsonFromBuffer(const fuchsia_mem::Buffer& buffer);

// NavListener watches webpage loading status.
class NavListener : public fidl::Server<fuchsia_web::NavigationEventListener> {
 public:
  // |fuchsia_web::NavigationEventListener|
  void OnNavigationStateChanged(OnNavigationStateChangedRequest& req,
                                OnNavigationStateChangedCompleter::Sync& completer) override;

  bool is_main_document_loaded_ = false;
  bool loaded_about_blank_ = false;
  bool window_resized_ = false;
  std::string title_;
};

// This base class serves as a bridge between the integration test and a web
// application running within a Fuchsia web frame. It allows sub-class to
// inject JavaScript, and use message port communication between this class and
// the web app.
class WebAppBase {
 public:
  WebAppBase();

 protected:
  // Parameters:
  // - `web_app_name`: only used in logs.
  // - `js_code`: injected to web page. It requires REGISTER_PORT/
  //    WINDOW_RESIZED handling in receiveMessage(), see mouse-input-chromium
  //    for example.
  // - `context_feature_flags`: the feature flags to create web context.
  void Setup(const std::string& web_app_name, const std::string& js_code,
             fuchsia_web::ContextFeatureFlags context_feature_flags =
                 fuchsia_web::ContextFeatureFlags::kVulkan |
                 fuchsia_web::ContextFeatureFlags::kNetwork);

  template <typename PredicateT>
  void RunLoopUntil(PredicateT predicate) {
    while (!predicate()) {
      loop_.Run(zx::time::infinite(), true);
    }
  }

  NavListener nav_listener_;
  // message_port used for JS send message to this class.
  fidl::Client<fuchsia_web::MessagePort> out_message_port_;

 private:
  void SetupWebEngine(const std::string& web_app_name,
                      fuchsia_web::ContextFeatureFlags context_feature_flags);
  void PresentView();
  void SetupWebPage(const std::string& js_code);
  void SendMessageToWebPage(fidl::ServerEnd<fuchsia_web::MessagePort> message_port,
                            const std::string& message);

  async::Loop loop_;
  component::OutgoingDirectory outgoing_directory_;
  fidl::ServerBindingGroup<fuchsia_web::NavigationEventListener> nav_listener_bindings_;
  fidl::SyncClient<fuchsia_web::ContextProvider> web_context_provider_;
  fidl::SyncClient<fuchsia_web::Context> web_context_;
  fidl::SyncClient<fuchsia_web::Frame> web_frame_;
  fidl::Client<fuchsia_element::GraphicalPresenter> presenter_;
};

}  // namespace integration_tests

#endif  // SRC_UI_TESTS_INTEGRATION_INPUT_TESTS_WEB_TEST_BASE_WEB_APP_BASE_H_
