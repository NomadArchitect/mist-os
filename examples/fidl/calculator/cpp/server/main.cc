// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// ============================================================================
// This is the C++ server which implements the fuchsia.examples.calculator protocol.
// The protocol is defined at //examples/fidl/calculator.test.fidl
// This component (and the accompying parent realm) is a realistic example of
// how to create & route client/server components in Fuchsia. It aims to be
// fully fleshed out and showcase best practices such as:
//
// 1. Integration testing
// 2. Exposing capabilities
// 3. Well commented code
// 4. FIDL interaction
// 5. Error handling
// 6. TODO(https://fxbug.dev/42062603) Unit testing
// ============================================================================

// Include the generated bindings for the Calculator protocol
#include <fidl/fuchsia.examples.calculator/cpp/fidl.h>
// Note: the pattern for the generated Natural bindings is:
// #include <fidl/<my.library.name>/cpp/fidl.h>
// For more information on the include path to the bindings, refer to:
// https://fuchsia.dev/fuchsia-src/development/languages/fidl/tutorials/cpp/basics/domain-objects?hl=en#include-cpp-bindings
#include <lib/async-loop/cpp/loop.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <math.h>

#include "fidl/fuchsia.examples.calculator/cpp/markers.h"

// For more information on the boilerplate for a fidl::Server, refer to:
// https://fuchsia.dev/fuchsia-src/development/languages/fidl/tutorials/cpp/basics/server#prerequisites
// The essential pattern here is that the local server implementation, |CalculatorServerImpl|,
// overrides the functions (which are generated by the FIDL backeng) that correspond to the FIDL
// protocol we're implementing.

class CalculatorServerImpl : public fidl::Server<fuchsia_examples_calculator::Calculator> {
 public:
  // Note: The Calculator protocol uses FIDL structs. For more information on
  // how to access the members of a struct, see:
  // https://fuchsia.dev/fuchsia-src/development/languages/fidl/tutorials/cpp/basics/domain-objects?hl=en#natural_structs

  // Adds two numbers together and returns their `sum`.
  void Add(AddRequest& request, AddCompleter::Sync& completer) override {
    FX_LOGS(INFO) << "Calculator server: " << __func__ << "() a=" << request.a()
                  << " b=" << request.b();
    // This type is actually an inline response struct. We created this local variable to be more
    // explicit about its type. For more detail on this, please see
    // https://fuchsia.dev/fuchsia-src/reference/fidl/bindings/cpp-bindings?hl=en#request-response-structs
    fidl::Response<::fuchsia_examples_calculator::Calculator::Add> my_response;
    // The member variables of my_response map to the corresponding FIDL method's response struct
    // defined in calculator.test.fidl
    my_response.sum() = request.a() + request.b();
    completer.Reply(my_response);
  }

  // Subtracts two numbers and returns their `difference`.
  void Subtract(SubtractRequest& request, SubtractCompleter::Sync& completer) override {
    FX_LOGS(INFO) << "Calculator server: " << __func__ << "() a=" << request.a()
                  << " b=" << request.b();
    auto difference = request.a() - request.b();
    // This is an inlined response struct, {{.difference = difference}}, with initiliazer list
    // style declaration. Compare to the above Add() response type, one could have also created a
    // fidl::Response<fuchsia_examples_calculator::Calculator::Subtract> and passed that to
    // completer.Reply() as was done above in the Add() function.
    // The member variables of this response struct map to the corresponding FIDL method's response
    // struct defined in calculator.test.fidl
    completer.Reply({{.difference = difference}});
  }

  // Multiplies two numbers and returns their `product`.
  void Multiply(MultiplyRequest& request, MultiplyCompleter::Sync& completer) override {
    FX_LOGS(INFO) << "Calculator server: " << __func__ << "() a=" << request.a()
                  << " b=" << request.b();
    auto product = request.a() * request.b();
    // Please see Add() and Subtract() for discussion on the response type. One could have also
    // created a fidl::Response<fuchsia_examples_calculator::Calculator::Multiply> and passed that
    // to completer.Reply()
    completer.Reply({{.product = product}});
  }

  // Divides one number by another and return the `quotient`.
  void Divide(DivideRequest& request, DivideCompleter::Sync& completer) override {
    FX_LOGS(INFO) << "Calculator server: " << __func__ << "()  dividend = " << request.dividend()
                  << " divisor=" << request.divisor();
    auto quotient = request.dividend() / request.divisor();
    // Please see Add() and Subtract() for discussion on the response type. One could have also
    // created a fidl::Response<fuchsia_examples_calculator::Calculator::Divide> and passed that to
    // completer.Reply()
    completer.Reply({{.quotient = quotient}});
  }

  // Takes `base` to the `exponent` and returns the `power`.
  void Pow(PowRequest& request, PowCompleter::Sync& completer) override {
    FX_LOGS(INFO) << "Calculator server: " << __func__ << "() base=" << request.base()
                  << " exponent=" << request.exponent();
    auto power = pow(request.base(), request.exponent());
    // Please see Add() and Subtract() for discussion on the response type. One could have also
    // created a fidl::Response<fuchsia_examples_calculator::Calculator::Pow> and passed that to
    // completer.Reply()
    completer.Reply({{.power = power}});
  }
};

int main(int argc, const char** argv) {
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"calculator_server"}).BuildAndInitialize();
  // The event loop is used to asynchronously listen for incoming connections
  // and requests from the client. The following initializes the loop, and
  // obtains the dispatcher, which will be used when binding the server
  // implementation to a channel.
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();

  // Create an |OutgoingDirectory| instance.
  //
  // The |component::OutgoingDirectory| class serves the outgoing directory for
  // our component. This directory is where the outgoing FIDL protocols are
  // installed so that they can be provided to other components.
  component::OutgoingDirectory outgoing = component::OutgoingDirectory(dispatcher);

  // The `ServeFromStartupInfo()` function sets up the outgoing directory with
  // the startup handle. The startup handle is a handle provided to every
  // component by the system, so that they can serve capabilities (e.g. FIDL
  // protocols) to other components.
  zx::result result = outgoing.ServeFromStartupInfo();
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve outgoing directory: " << result.status_string();
    return -1;
  }

  // Create an actual instance of the server.
  std::unique_ptr server_ptr = std::make_unique<CalculatorServerImpl>();

  // This is the most straightforward way to add the protocol to this component's outgoing "served"
  // capabilities - we pass it the server instance (which overrides fidl::Server) and it calls
  // fidl::BindServer() for us.
  result = outgoing.AddProtocol<fuchsia_examples_calculator::Calculator>(std::move(server_ptr));

  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to add Calculator protocol: " << result.status_string();
    return -1;
  }

  FX_LOGS(INFO) << "C++ calculator server has started!";

  // This runs the event loop and blocks until the loop is quit or shutdown.
  // See documentation comments on |async::Loop|.
  loop.Run();
  return 0;
}
