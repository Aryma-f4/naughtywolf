# Module Studio

Module Studio lives on each callback detail page and sends its work through the same encrypted task and result channel as ordinary commands. It is intended for systems you own or are authorized to assess.

## User enumeration

**Enumerate users** queues `nw/user-enum`. On Unix hosts it reads the local account database, marks shells that permit login, records the current user, and captures active login sessions. On Windows it uses the built-in `Get-LocalUser` PowerShell cmdlet with a fixed script. The collector is read-only and accepts no operator-controlled arguments.

## PEASS assessment

**Run assessment** queues `nw/peas-audit`. The implant selects `linpeas.sh` on Linux or `winPEASany_ofs.exe` on Windows and downloads the current asset from the [official PEASS-ng releases](https://github.com/peass-ng/PEASS-ng/releases/latest). Redirects are accepted only for GitHub-owned release hosts.

Before returning the assessment output, NaughtyWolf records:

- the final download URL;
- the artifact SHA-256 digest;
- unique CVE references found in the output;
- confirmation that automatic exploitation is disabled.

The callback host needs outbound HTTPS access to GitHub. Downloads are capped at 64 MiB, task output at 2 MiB, and the temporary PEASS file is deleted after the run. Findings are possible escalation paths for operator review; the module does not launch exploits or change account privileges.

## Custom code

The editor queues `nw/exec-code` with the interpreter name and UTF-8 source encoded as base64. The implant writes the source to a private temporary file and invokes the interpreter directly, without embedding source text in a shell command.

| Target | Interpreters |
| --- | --- |
| Linux and macOS | Shell, Python 3 |
| Windows | PowerShell, Python |

Source is capped at 24 KiB and output at 2 MiB. The selected task timeout applies to execution, and the temporary source file is removed after completion or timeout.

## Operational flow

1. Open **Active Callbacks** and select **Interact**.
2. Choose a quick assessment or open the custom-code editor.
3. Watch the task ledger for queued, running, and completed state.
4. Review PEASS candidates and supporting output before deciding on any next action.
