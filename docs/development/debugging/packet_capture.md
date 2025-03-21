# Packet Capture on Fuchsia

Packet capture is a fundamental tool for developing, debugging, and testing networking.

`fx sniff` is a development host command that:

* Runs the packet capture on the Fuchsia **target** device.
* Stores the packets in PCAPNG format on the Fuchsia development **host**.
* Streams out to a graphical user interface such as `Wireshark`.

`tcpdump` is a packet capturer with rich capture filter support. `fx sniff` internally invokes `tcpdump` with predefined capture filters that are necessary for Fuchsia's developer workflow. For use cases where `fx sniff` is not viable (e.g. when you have serial console access but without dev host connected), use `tcpdump` directly.

## Prepare the image {#prepare-image}

Make sure to bundle `tcpdump` into your set of base packages.

```shell
$ fx set core.x64 --with-base //third_party/tcpdump
$ fx build
```

## How-to (On Host)

### Capture packets over WLAN interface

```shell
[host] $ fx sniff wlan
```

By default, this command captures packets for 30 seconds. To configure the duration, add the `--time {sec}` or `-t {sec}` option.

If you don't know the network interface name, run `fx sniff` without options. The error message shows you what interfaces are available. Alternatively, run:

```shell
[host] $ fx shell net if list
```

### Show the hexdump of packets over the ethernet interface

```shell
[host] $ fx sniff --view hex eth
```

### Capture WLAN packets and store them in a file

```shell
[host] $ fx sniff --file my_packets wlan
```

The captured packets are first stored in the target's `/tmp/` directory. After the capture is complete, the files are moved to `//out/my_packets.pcapng` automatically.

### Stream out to Wireshark in realtime

**_NOTE:_** Linux only.

```shell
[host] $ fx sniff --view wireshark wlan
```

### Force stop
Packet capture runs for the specified duration (`--time` or `-t` option). If a user desires to stop early, presse one of the following keys:

```
c, q, C, Q
```
This will stop both a target side process and a host side process.

## How-to (on target device)

### Use tcpdump for debugging

`fx sniff` requires working `ssh` connectivity from the host to the target, which means that networking must be working to some degree. In some cases, networking might not be working at all. If you have access to the serial console while networking, including `ssh`, is not working, you must run `tcpdump` directly on the target. `tcpdump` provides a richer set of features than `fx sniff`.

#### Capture packets over the WLAN interface

```shell
[target] $ tcpdump -i wlan --no-promiscuous-mode
```

#### Stream out the binary dump in PCAPNG format

```shell
[target] $ tcpdump -i wlan --no-promiscuous-mode -w -
```

#### Capture packets and store them in a file

```shell
[target] $ tcpdump -i wlan --no-promiscuous-mode -w /tmp/my_packets.pcapng
```

#### Copy the dump file to the host

```shell
[host] $ cd ${FUCHSIA_OUT_DIR} && fx scp "[$(fx get-device-addr)]:/tmp/my_packets.pcapng"
```

#### `tcpdump` help

```shell
[target] $ tcpdump --help
```

#### Only Watch ARP, DHCP, and DNS packets

```shell
[target] $ tcpdump -i  wlan --no-promiscuous-mode "arp or port dns,dhcp" "$iface_filepath"
```

## Filter syntax
`tcpdump` uses `libpcap` under the hood. See [pcap-filter](https://www.tcpdump.org/manpages/pcap-filter.7.html).

## Reference: `fx` workflow packet signatures
There are many different kinds of services running between the Fuchsia
development host and the target. Those are usually invoked by `fx` commands.
Most of times, you are not interested in those packets generated by the `fx`
workflows. The following table lists noteworthy signatures.

| Use                  | Signature                    | Reference                                  |
|----------------------|------------------------------|--------------------------------------------|
| Logger               | port 33337                   | NETBOOT_DEBUGLOG_PORT_SERVER               |
| Logger               | port 33338                   | NETBOOT_DEBUGLOG_PORT_ACK                  |
| Bootserver           | port 33330                   | NETBOOT_PORT_SERVER                        |
| Bootserver           | port 33331                   | NETBOOT_PORT_ADVERT                        |
| Bootserver           | port 33332                   | NETBOOT_PORT_CMD_START                     |
| Bootserver           | port 33339                   | NETBOOT_PORT_CMD_END                       |
| Bootserver           | port 33340                   | NETBOOT_PORT_TFTP_OUTGOING                 |
| Bootserver           | port 33341                   | NETBOOT_PORT_TFTP_INCOMING                 |
| Package Server       | port 8083                    | docs/packages.md                           |
| fx shell             | port 22                      | devshell/shell                             |
| target netsvc addr   | fe80::xxxx:xxff:fexx:xxxx%XX | fx device-finder list --netboot            |
| host link-local addr | fe80::xxxx:xxxx:xxxx:xxxx%XX | fx device-finder list --ipv4=false --local |
| target netstack addr | fe80::xxxx:xxxx:xxxx:xxxx%XX | fx get-device-addr                         |
| zxdb                 | port 2345                    | devshell/contrib/debug                     |
| -                    | port 65026                   |                                            |
| -                    | port 65268                   |                                            |
| -                    | 1900                         |                                            |


## Troubleshooting

**_Q_** I get the error `/boot/bin/sh: tcpdump not found`

**A** The `tcpdump` package is not prepared. Make sure to bundle `tcpdump` in the image. See [prepare the image](#prepare-image).
