# More machines

Pairing connects the Razor servers on two machines, each holding a key for the other, so that the terminal app on either shows the sessions, questions and deliveries of both, and an agent on one can start a helper on the other or send it mail. Each server keeps its own sessions and records; pairing lets them reach each other.

Paired servers talk plain HTTP with a bearer key on each request, so they must reach each other over a private network that encrypts traffic and decides who can connect. [Tailscale](https://tailscale.com) does both and is what this page uses.

## 1. Put the machines on Tailscale

Install Tailscale on each machine, sign in to the same Tailscale network, and note each machine's Tailscale address:

```sh
tailscale ip -4
```

## 2. Give each server an address

On each machine, set the address its server listens on, its Tailscale address and a port of your choice, and a short name for the machine, which also starts the name of each new session you start there:

```toml
# ~/.razor/config.toml on the desk machine
[server]
address = "100.101.102.103:7433"
name = "desk"
```

```toml
# ~/.razor/config.toml on the laptop
[server]
address = "100.104.105.106:7433"
name = "laptop"
```

Run `razor restart` on each machine ([Troubleshooting](troubleshooting.md#restarting-the-server)); `server.address` applies only at start. Use the Tailscale address, never `0.0.0.0` or a public one: anyone who can reach the port can try keys against it.

## 3. Pair them

On the desk, add the laptop by the name the desk will call it and the laptop's address:

```sh
razor devices add laptop --address 100.104.105.106:7433
```

It prints a command for the other side, carrying a new key:

```text
laptop is paired. On laptop, run this, with the name laptop will call this server:
razor devices add <name> --address 100.101.102.103:7433 --key 3f1c…
```

Run that on the laptop, with `<name>` replaced by what the laptop calls the desk:

```sh
razor devices add desk --address 100.101.102.103:7433 --key 3f1c…
```

A name is 1 to 32 lowercase letters, digits and hyphens. `razor devices` lists every paired server and enrolled program by name, and `razor devices remove <name>` removes one: its key stops working at once. Without `--address`, `razor devices add <name>` enrolls a client program rather than a server and shows its key once; treat that key like a password, since it can do anything the local terminal can.

## What pairing gives you

| What | How it works |
| --- | --- |
| One list | Each machine's terminal app lists the other's sessions, questions and inbox beside its own, and opens, steers and stops them as if they were local. |
| Helpers on another machine | An agent can start a helper on a paired server by naming it; the helper works in that machine's checkout, and merging its branch happens there. |
| Mail across machines | Any agent can mail any other on a paired server, by name or session id. |
| Shared settings | One server can keep the saved arrangements of the agents list (`/views`) and the voice settings for all of them: set `[server] shared` on the others to its id, the contents of `server.id` in its Razor home. |

A server that stops answering keeps its last rows in the list, marked with how long since it was heard from. A paired server's key is a full key: it can do on this server anything this server's own terminal can, devices, settings and sign-ins included, so pair only machines you trust as you trust this one. Each pairing is direct: a server shows the servers it has paired, not their pairs.

## More than one Razor on one machine

Each Razor home runs its own server, so one machine can hold two separate Razors, each with its own settings, keys, ChatGPT sign-in and sessions (Claude's sign-in belongs to your user account, so they share it), by pointing `RAZOR_HOME` at a second folder:

```sh
RAZOR_HOME=~/.razor-second razor
```

Give each home its own `[server] address` port if both should be reachable. `razor install` registers each home's background service under its own name, the home's folder without its leading dot (`razor` for `~/.razor`, `razor-second` for `~/.razor-second`), so each home can run as a service; it never replaces a service of the same name that runs another home.
