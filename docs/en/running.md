# Run Crucible Core

## Does it have a UI?

Crucible Core is a headless Rust library workspace. A host application owns the process and drives the simulation. A graphical client owns rendering, input, and on-screen controls. To connect a separate client, the host supplies a protocol adapter that converts input and replicated state between the engine and the client.

The source snapshot includes the [administrator client](admin.md) for starting and stopping a host, inspecting its logs, and monitoring tick and mechanic timings. Use the [extension walkthrough](extensions.md) to build a small host and display its telemetry in the console. A networked game requires a host with a transport and a separate game client.

## Run the example as a Linux user service

Build, check, and install `crucible-demo-host` by following the [extension walkthrough](extensions.md). Create the `~/.config/systemd/user` directory and save this unit as `crucible-demo-host.service`.

```ini
[Unit]
Description=Crucible Core demo host

[Service]
Type=exec
WorkingDirectory=%h/src/crucible-demo-host
ExecStart=%h/.local/bin/crucible-demo-host
Restart=on-failure
RestartSec=2
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=default.target
```

The unit launches the compiled host directly and restarts it after a failure. `%h` is the user's home directory. Adjust `WorkingDirectory` if you built the host elsewhere. [systemd service reference](https://www.freedesktop.org/software/systemd/man/latest/systemd.service.html)

```sh
systemctl --user daemon-reload
systemctl --user enable --now crucible-demo-host.service
systemctl --user status crucible-demo-host.service
journalctl --user -u crucible-demo-host.service -f
```

On a headless server, have the administrator enable lingering for the account if the service must start at boot and keep running after logout. [loginctl reference](https://www.freedesktop.org/software/systemd/man/latest/loginctl.html#enable-linger%20USER%E2%80%A6)

```sh
loginctl enable-linger "$(id -un)"
```

Use `systemctl --user stop crucible-demo-host.service` to stop the host. To stop it and disable automatic startup, run the following command. [systemctl reference](https://www.freedesktop.org/software/systemd/man/latest/systemctl.html)

```sh
systemctl --user disable --now crucible-demo-host.service
```
