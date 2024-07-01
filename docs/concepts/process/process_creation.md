# Process Creation

The kernel provides low-level facilities for creating and setting up processes.
However, these facilities are difficult to use because they involve directly
mapping memory for executables, shared libraries, and stacks. Instead, you should
use one of the higher-level mechanisms for creating processes.

## fuchsia.process.Launcher

Fuchsia provides a service, `fuchsia.process.Launcher`, that does the low-level
work of constructing processes for you. You provide this service with all the
kernel objects needed to construct the process (e.g., the job object in which
the process should be created, the executable image, and the standard input and
output handles), and the service does the work of parsing the ELF executable
format, configuring the address space for the process, and sending the process
the startup message.

Most clients do not need to use this service directly. Instead, most clients can
use the simple C frontend in the FDIO library called `fdio_spawn`. This
function, and its more advanced `fdio_spawn_etc` and `fdio_spawn_vmo`
companions, connect to the `fuchsia.process.Launcher` service and send the
service the appropriate messages to create the process.  The
`fdio_spawn_action_t` array passed to `fdio_spawn_etc` can be used to customize
the created process.

Regardless of whether you use the `fuchsia.process.Launcher` service directly
or the `fdio_spawn` frontend, this approach to creating processes is most
appropriate for creating processes within your own namespace because you need
to supply all the kernel objects for the new process.

## Early boot

Early on in the boot process, the system does create a number of processes
manually. For example, the kernel manually creates the first userspace process,
`userboot`.

Userboot's most important job is to load the next process from the bootfs image
in the ZBI, which by default is `component_manager`.

Direct construction of processes (such as how `userboot` loads
`component_manager`) is prohibited in the `fuchsia` job tree using a job
policy. Libraries or programs that might be used from the `fuchsia` job tree
may use `fdio_spawn` (or its companions) to create processes while conforming
to the security policy.
