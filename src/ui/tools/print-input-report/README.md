# `print-input-report`

A utility that can attach itself to an input device and read the device
descriptors or events.

## Testing

```
fx test //src/ui/tools/print-input-report
```

## Example use

From the host command line:

```
fx shell print-input-report descriptor /dev/class/input-report/002
```

## Get more help

The invocation below will print usage.

```
fx shell print-input-report
```
