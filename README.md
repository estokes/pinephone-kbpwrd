# PinePhone (Pro) Keyboard Case Power Manager

As described in Megi's blog, the dual battery nature of the keyboard
case and the PinePhone present some power management challenges that
need to be solved in software. This little daemon is meant to get
optimal run times out of the system, as well as respond to changes in
charging state.

## Theory of Operation

Moving charge around is an inherently wasteful thing to do, one looses
energy at the step up to usb voltage, the step down from usb to charge
voltage, and in the charging itself. The most efficient thing to do in
a dual battery system is for each battery to contribute to powering
the load in proportion to it's capacity, thereby eliminating at least
the charging losses (the step up and step down is unavoidable). As
such this daemon seeks to do exactly that, within the limits of the
hardware.

## Current State

At the moment it is only working on the pinephone pro, mainly because
that is it's development platform, but also because the PPP has more
need of it due to it's higher power consumption. I do plan to make it
work on the pinephone at some point.

## Installation

If you have rust installed you can build it with

```
$ cargo install kbpwrd
```

I may post binaries for poplar distros. Hopefully it will get
packaged. Because this software runs as root, you should read it, or
have someone you trust read it before running it. I know there have
been malware incidents in the pine community recently. This isn't
malware, but don't take my word for it!

## Use

Right now it's in an early stage of development and testing, so it
doesn't daemonize, and I didn't write systemd startup units or init
scripts yet. I run it as root in a terminal with logging turned on.

```
[root@sasami-chan ~]$ RUST_LOG=info {path-to-binary}/kbpwrd
```

it will then print log messages every second

```
2022-02-18T19:28:12Z INFO  kbpwrd] ph v: 4181, c: -192, s: Discharging, l: 450, kb v: 3912, c: -614, s: Discharging, l: 2300, act: Pass

```

currents are in mA and voltages are in mV. 'l' is the input current
limit. The first part of the line referrs to the phone, the second to
the keyboard, and the action describes what the daemon plans to do
this cycle (e.g. raise, lower, set to default the input current limit
on the phone).

When running the daemon you shoul observe your main battery
discharging, but much more slowly than it normally would. Due to
better power management and the discrete steps that input current
limit can accept the keyboard battery will take a larger portion of
the load than it should, and as such it will likely discharge first.
