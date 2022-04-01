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

It seeks to both avoid charging the internal battery with the keyboard
battery, and to keep both batteries at roughly the same state of
charge, by changing the input current limit of the internal battery in
response to the current load and a set of heristics. The only
exception to this rule is that it will charge the internal battery if
it falls below 20%, to prevent the phone from completely discharging
while the keyboard battery still has capacity (quite critical on the
PPP, as a complete discharge there can mean spending hours in maskrom
mode before a bootup is possible).

There are a few benefits to this approach,

- LiPo batteries (and batteries in general) are more efficient at
  light loads relative to their capacity, so we should get longer
  runtimes at e.g. C/10 vs at C/2
- To some extent this is also true for power electronics
- Since we roughly match the state of charge of the keyboard to the
  phone, the phone's fuel gauge is a reasonable approximation of the
  actual state of charge.

## Current State

Now working on the pinephone and pinephone pro. The pinephone has an
unfortunate kernel bug that causes it to report the absolute value of
the current instead of the actual value. This issue combined with the
problem that the battery state is always Charging if a charger is
connected, even if the battery is actually discharging, means that I
have to use a heuristic to guess when the battery is discharging. It
works fine most of the time, but there will be cases where I guess
wrong. This isn't as bad as it sounds, since the default limit of
500mA is almost always the correct value for the pinephone.

The powerbank ic in use in the keyboard does not really deal well with
balacing charging it's own battery and feeding the load. If you leave
it at the default current limit and increase the phone current limit
when both batteries are deeply discharged it will draw too much
current and shutdown/restart in a loop while getting pretty warm. The
daemon manages the limits when charging in order to prevent this from
happening. It will always try to keep both batteries charging, but
will prioritize the main battery so the phone doesn't run out of power
and turn off. It tries to keep within safe limits, and as a result
might not charge quite as fast as would be technically possible.

## Todo

- Guess the keyboard battery state of charge in order to make a better
  descision about which battery should take the load

- Gather more runtime data

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
2022-04-01T01:34:46Z INFO  kbpwrd] ph v: 3923, a: 469, s: Charging, l: 850, c: 47, kb v: 4071, a: 1527, s: Charging, l: 1500, c: 48, act: Pass

```

currents are in mA and voltages are in mV. 'l' is the input current
limit. The first part of the line referrs to the phone, the second to
the keyboard, and the action describes what the daemon plans to do
this cycle (e.g. raise, lower, set to default the input current limit
on the phone).

When running the daemon you should observe your main battery
discharging, but much more slowly than it normally would.
