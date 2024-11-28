# Zircon Transport Rust Example

Reviewed on: 2024-03-21

This example demonstrates a parent driver serving the fuchsia.hardware.i2c FIDL
protocol over zircon transport and a child driver that connects to the protocol
to interact with the parent driver, with both drivers written in rust.

## Building

To include the driver to your build, append `--with //examples/drivers:drivers`
to your `fx set` command. For example:

```bash
$ fx set core.x64 --with //examples/drivers:drivers
$ fx build
```

## Running

Register the parent driver by running this command
```bash
$ ffx driver register fuchsia-pkg://fuchsia.com/zircon_transport_rust#meta/zircon_transport_parent.cm
```

Register the child driver by running this command
```bash
$ ffx driver register fuchsia-pkg://fuchsia.com/zircon_transport_rust#meta/zircon_transport_child.cm
```

Verify that both drivers show up in this command
```bash
ffx driver list
```

Add a test node that binds to the parent driver:
```bash
$ ffx driver test-node add zircon_transport_rust_parent gizmo.example.TEST_NODE_ID=zircon_transport_rust_parent
```

Run the following command to verify that the driver is bound to the node:
```bash
$ ffx driver list-devices -v transport
```

You should see something like this:
```
Name     : zircon_transport_rust_parent
Moniker  : dev.zircon_transport_rust_parent
Driver   : fuchsia-pkg://fuchsia.com/zircon_transport_rust#meta/zircon_transport_parent.cm
2 Properties
[ 1/  2] : Key "gizmo.example.TEST_NODE_ID"   Value "zircon_transport_rust_parent"
[ 2/  2] : Key "fuchsia.platform.DRIVER_FRAMEWORK_VERSION" Value 0x000002
0 Offers

Name     : zircon_transport_rust_child
Moniker  : dev.zircon_transport_rust_parent.zircon_transport_rust_child
Driver   : fuchsia-pkg://fuchsia.com/zircon_transport_rust#meta/zircon_transport_child.cm
2 Properties
[ 1/  2] : Key "fuchsia.hardware.i2c.Service" Value "fuchsia.hardware.i2c.Service.ZirconTransport"
[ 2/  2] : Key "fuchsia.platform.DRIVER_FRAMEWORK_VERSION" Value 0x000002
1 Offers
Service: fuchsia.hardware.i2c.Service
  Source: dev.zircon_transport_rust_parent
  Instances: default

Name     : transport-child
Moniker  : dev.zircon_transport_rust_parent.zircon_transport_rust_child.transport-child
Driver   : unbound
2 Properties
[ 1/  2] : Key "fuchsia.test.TEST_CHILD"      Value "rust i2c server"
[ 2/  2] : Key "fuchsia.platform.DRIVER_FRAMEWORK_VERSION" Value 0x000002
0 Offers
```

## Testing

Include the tests to your build by appending `--with //examples/drivers:tests` to your `fx
set` command. For example:

```bash
$ fx set core.x64 --with //examples/drivers:drivers --with //examples:tests
$ fx build
```

Run unit tests with the command:
```bash
$ fx test zircon-rust-driver
```

This will run unit tests against both the parent and child driver implementations.

## Source layout

The core implementation of the parent driver is in `parent/src/*.rs` and the child driver
is in `child/src/*.rs`.
