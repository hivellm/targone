# Spike 0.6 — scheduler registration

> Executed 2026-08-18 on the reference machine (Windows 10 Pro 10.0.19045,
> pt-BR locale, user `BOLADO\Bolado`, **non-elevated** PowerShell 7). All
> Windows claims below are captured command output from disposable tasks
> (`TargoneSpikeTest`, `TargoneSpikeIdle`, `TargoneSpikeTestPS`) — all three
> deleted afterwards, final sweep count 0. Linux claims verified against the
> systemd 255 man pages shipped in the local WSL Ubuntu-24.04 instance plus a
> live WSL observation. macOS claims are documentation-only (no machine
> available); sources cited inline. Experiment script: session scratchpad,
> `spike06-scheduler-experiments.ps1`.

## Verdict

### Windows — Task Scheduler via the COM/`TaskService` API (what the PowerShell cmdlets wrap), per-user task, no elevation

- **Registration mechanism:** register `\Targone` (or `\Targone\Sweep`) as a
  per-user task with a daily `CalendarTrigger` + `RunOnlyIfIdle` settings —
  the exact combination "daily AND only-if-idle" that `schtasks.exe` cannot
  express but the Task Scheduler API can. From Rust, use the
  `ITaskService`/`ITaskDefinition` COM interfaces (crate `windows`), which is
  what `Register-ScheduledTask` wraps; the spike proved the API accepts it
  non-elevated. Set `AllowStartIfOnBatteries`/`DontStopIfGoingOnBatteries`
  (i.e. `DisallowStartIfOnBatteries=false`, `StopIfGoingOnBatteries=false`)
  explicitly — **the defaults are battery-hostile** (won't start on battery,
  kills the task if AC is unplugged). Confidence: high (all empirical).
- **Rights needed:** none beyond the user's own login. Create, overwrite,
  query, on-demand run, and delete of a per-user `InteractiveToken` task all
  succeeded non-elevated, **no password prompted or stored**. Confidence:
  high (empirical).
- **Interactive-only caveat:** the no-password default is
  `LogonType=InteractiveToken` — the task runs **only while the user is
  logged on**. Upgrading to run-while-logged-out costs something: S4U
  registration was **denied non-elevated** (`Access denied` — empirical), and
  password-based (`/RU`+`/RP`) registration requires capturing the user's
  password. Targone should ship logged-on-only as the default and treat
  run-while-logged-out as an opt-in that asks for elevation (S4U) —
  acceptable, because a sweep scheduled for an idle logged-on machine is the
  actual use case. Confidence: high for the observed denial; medium for the
  generalization that S4U always needs elevation (single machine, default
  policy).
- **Idempotent re-registration recipe:** register with overwrite semantics
  (`TASK_CREATE_OR_UPDATE` in the API / `-Force` in the cmdlet / `/F` in
  schtasks) keyed on the fixed task path `\Targone`. Empirically a clean
  atomic replace; without the overwrite flag schtasks **prompts
  interactively (Y/N)** — hangs automation — and the cmdlet errors with
  `0x800700B7` ("file already exists"). So: always overwrite, never
  create-if-absent. Confidence: high (empirical).
- **No-rights degradation:** none needed for the core path — it already
  works without admin. If the Task Scheduler service is disabled by policy
  (not testable here), degrade to opportunistic sweeps piggybacked on normal
  `cargo targone` invocations plus a nag in `status`. Confidence: high that
  no elevation is needed; low on the policy-disabled edge (untested).

### Linux — systemd user timer; cron fallback

- **Registration mechanism:** write
  `~/.config/systemd/user/targone-sweep.{service,timer}` (path verified in
  systemd.unit(5) "User Unit Search Path"), then
  `systemctl --user daemon-reload && systemctl --user enable --now
  targone-sweep.timer`. Timer: `OnCalendar=daily` +
  `RandomizedDelaySec=1h` + `Persistent=true` (catch up a missed run at next
  user-manager start — verified semantics, systemd.timer(5)). Service:
  `Nice=19`, `CPUSchedulingPolicy=idle`, `IOSchedulingClass=idle`
  (systemd.exec(5)) — systemd has **no "run only when user is idle"
  condition**, so "idle" is approximated by idle-class scheduling;
  battery gating is `ConditionACPower=true` on the service
  (systemd.unit(5), verified quote) — leave it unset by default and expose
  it as config. Confidence: high (man-page-verified against systemd 255);
  the "no user-idle condition exists" claim is medium (absence of evidence
  in systemd.timer(5)/systemd.unit(5), not a positive statement).
- **Rights needed:** none — user units are the user's own files and
  `systemctl --user` talks to the user's own manager. Run-while-logged-out
  additionally needs `loginctl enable-linger` (loginctl(1): keeps a user
  manager "around after logouts"); calling it for *yourself* may hit a
  polkit prompt depending on distro policy — treat as opt-in, not required.
  Confidence: high for the mechanism; medium for polkit variability.
- **Idempotent re-registration:** unit files are plain files at fixed paths —
  overwrite + `daemon-reload` + `enable --now` is naturally idempotent
  (`enable` on an already-enabled unit is a no-op). Removal:
  `systemctl --user disable --now targone-sweep.timer`, delete the two
  files, `daemon-reload`; systemd.timer(5) also documents
  `systemctl clean --what=state` to drop the `Persistent=` stamp file
  before uninstall. Confidence: high.
- **No-rights / no-systemd degradation:** `systemctl --user is-system-running`
  is the probe — on this machine's WSL it returns `offline` /
  "Failed to connect to bus" because PID 1 is WSL's own init (observed).
  Fallback chain: (1) user crontab (`crontab -l` merge + `crontab -`) — but
  observed in WSL that even the cron daemon isn't running by default, so
  (2) final fallback is the same opportunistic-sweep-on-invocation path as
  Windows. Never require root. Confidence: high (WSL observed; cron
  fallback semantics are standard but untested here).

### macOS — launchd user LaunchAgent (documentation-only; no test machine)

- **Registration mechanism:** write
  `~/Library/LaunchAgents/dev.targone.sweep.plist` (file must be owned by
  the user, mode 600/400 — Apple "Creating Launchd Jobs") with
  `StartCalendarInterval` for daily runs, `ProcessType=Background`,
  `LowPriorityIO`/`Nice` for politeness; load with
  `launchctl bootstrap gui/$UID ~/Library/LaunchAgents/dev.targone.sweep.plist`
  (modern replacement for legacy `load`, per launchctl(1)); on-demand test
  run via `launchctl kickstart gui/$UID/dev.targone.sweep`. Missed-run
  behavior is favorable: launchd.plist(5) — "Unlike cron which skips job
  invocations when the computer is asleep, launchd will start the job the
  next time the computer wakes up", with multiple missed intervals
  "coalesced into one event". Confidence: medium-high (authoritative docs,
  zero local verification).
- **Rights needed:** none documented for the user's own `gui/$UID` domain —
  launchctl(1) requires root only for *system*-domain modifications; agents
  run "only while that user is logged in" (Apple), so run-while-logged-out
  needs a root-owned LaunchDaemon — same opt-in/elevation posture as
  Windows S4U. **Idle/battery: launchd.plist has no idle or AC-power
  condition keys** — `ProcessType=Background` resource-throttling is the
  only lever (launchd.plist(5)). Confidence: medium (documented absence).
- **Idempotent re-registration:** overwrite the plist, then
  `launchctl bootout gui/$UID/dev.targone.sweep 2>/dev/null;
  launchctl bootstrap gui/$UID <plist>` — bootout-then-bootstrap because
  bootstrap of an already-loaded service errors. Confidence: medium
  (documented pattern; exact error behavior unverified).
- **Degradation:** if `launchctl bootstrap` fails (MDM policy, sandbox),
  same opportunistic-sweep fallback. Behavior on power-off (not sleep) over
  a scheduled time is **not** covered by the cited text — flagged unknown,
  verify on real hardware before relying on catch-up. Confidence: low on
  power-off catch-up (deliberately unclaimed).

### Cross-platform shape for `cargo targone schedule install`

One fixed identity per platform (`\Targone` task, `targone-sweep.timer`,
`dev.targone.sweep`), overwrite-style registration so re-running `install`
is always safe, `schedule uninstall` as the exact inverse, `schedule status`
probing the platform scheduler, and a shared fallback: when no scheduler is
usable, record intent in Targone's own config and sweep opportunistically on
normal invocations. No path requires elevation; the two features that would
(run-while-logged-out on Windows, LaunchDaemon on macOS) are explicit
opt-ins, not defaults.

## Evidence — Windows (empirical)

Shell state: `Elevated (admin): False`, user `BOLADO\Bolado`, Windows
10.0.19045. Locale is pt-BR — `ÊXITO` = SUCCESS, `ERRO` = ERROR, <!-- codespell:ignore erro -->
`AVISO` = WARNING in the captures below.

### 1. Non-elevated create — succeeds, no password

```
> schtasks /Create /TN TargoneSpikeTest /TR "cmd /c exit 0" /SC DAILY /ST 03:00 /F
ÊXITO: a tarefa agendada "TargoneSpikeTest" foi criada corretamente.   [exit 0]
```

Verbose query (key lines, translated in brackets):

```
Modo de Logon:            Interativo apenas          [Logon mode: Interactive only]
Executar como Usuário:    Bolado                     [Run as user: Bolado]
Tempo Ocioso:             Desativado                 [Idle condition: Disabled]
Gerenciamento de Energia: Parar se estiver usando baterias,
                          Não iniciar se estiver usando baterias
                          [Stop if on batteries; Do not start if on batteries]
Último resultado:         267011                     [0x41303 = task has not yet run]
```

Exported XML (defaults that matter — task format version 1.2):

```xml
<Principal id="Author">
  <UserId>S-1-5-21-...-1001</UserId>
  <LogonType>InteractiveToken</LogonType>
</Principal>
<Settings>
  <DisallowStartIfOnBatteries>true</DisallowStartIfOnBatteries>
  <StopIfGoingOnBatteries>true</StopIfGoingOnBatteries>
  <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
  <IdleSettings>
    <Duration>PT10M</Duration> <WaitTimeout>PT1H</WaitTimeout>
    <StopOnIdleEnd>true</StopOnIdleEnd> <RestartOnIdle>false</RestartOnIdle>
  </IdleSettings>
</Settings>
```

No password was prompted or supplied at any point in the spike.

### 2. Idempotency

```
> schtasks /Create ... /F          (task exists)
ÊXITO: ... criada corretamente.                                        [exit 0 — clean overwrite]

> schtasks /Create ...             (no /F, task exists)
AVISO: já existe uma tarefa com o nome "TargoneSpikeTest". <!-- codespell:ignore nome -->
Deseja substituí-la (S/N)?                                             [exit 1]
```

Without `/F` schtasks does **not** error — it asks an interactive Y/N
question ("a task with this name already exists. Replace it?"). In a
non-interactive shell (stdin at null) it read EOF and exited 1; in a real
automation context it can hang. Register-ScheduledTask behaves better:
`-Force` overwrites cleanly (verified — changed the trigger 03:00→03:30),
without `-Force` it raises a proper non-interactive error:

```
Não é possível criar um arquivo já existente.       [0x800700B7 ERROR_ALREADY_EXISTS]
```

### 3. Idle triggers

`/SC ONIDLE` is allowed non-elevated (exit 0) but produces a bare
`<IdleTrigger/>` — fires whenever the machine goes idle, no calendar
component; `/I 10` only fed `IdleSettings/Duration`. schtasks.exe has no way
to attach an idle *condition* to a daily trigger.

The API path does it non-elevated:

```
> New-ScheduledTaskSettingsSet -RunOnlyIfIdle -IdleDuration 00:10:00 -IdleWaitTimeout 01:00:00
    -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
> Register-ScheduledTask -TaskName TargoneSpikeTestPS -Action ... -Trigger (Daily 03:00) -Settings ...
REGISTERED: TargoneSpikeTestPS State=Ready
```

Exported XML confirms the combination (task format 1.3):

```xml
<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
<RunOnlyIfIdle>true</RunOnlyIfIdle>
<IdleSettings><Duration>PT10M</Duration><WaitTimeout>PT1H</WaitTimeout>
  <StopOnIdleEnd>true</StopOnIdleEnd>...</IdleSettings>
...
<CalendarTrigger><StartBoundary>2026-08-18T03:00:00-03:00</StartBoundary>
  <ScheduleByDay><DaysInterval>1</DaysInterval></ScheduleByDay></CalendarTrigger>
```

Note `StopOnIdleEnd=true` default: the sweep gets killed when the user comes
back — for Targone that is a *feature* (politeness), but the sweep must be
resumable/idempotent mid-flight (consistent with spike 0.1/0.6 safety work).

### 4. Battery/AC defaults

Captured above: schtasks defaults are `DisallowStartIfOnBatteries=true` +
`StopIfGoingOnBatteries=true`. Disabled non-elevated via
`New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries
-DontStopIfGoingOnBatteries` (XML flipped to `false`/`false`, verified).

### 5. On-demand run

```
> schtasks /Run /TN TargoneSpikeTest
ÊXITO: tentativa de executar a tarefa agendada "TargoneSpikeTest".     [exit 0]
> Get-ScheduledTaskInfo TargoneSpikeTest
LastRunTime: 08/18/2026 04:51:51    LastTaskResult: 0 (0x0)
```

Non-elevated on-demand run works and status fields update — usable as the
verification step at the end of `schedule install`.

### S4U (run whether logged in, passwordless) — denied non-elevated

```
> New-ScheduledTaskPrincipal -UserId BOLADO\Bolado -LogonType S4U
> Register-ScheduledTask -TaskName TargoneSpikeTestPS ... -Principal $principal -Force
S4U FAILED: Acesso negado.                          [Access denied]
```

The failure did not corrupt the existing task (still `LogonType=Interactive`
afterwards). So the interactive-vs-background matrix on this machine:
InteractiveToken = no password, no elevation, logged-on-only;
S4U = no password but **requires elevation**; password logon (`/RU`+`/RP`) =
untested (would require capturing the user's password — rejected as a
design option, not just untested).

### 6. Removal — clean, non-elevated, verified

```
> schtasks /Delete /TN TargoneSpikeTest /F
ÊXITO: a tarefa agendada "TargoneSpikeTest" foi excluída corretamente. [exit 0]
> schtasks /Query /TN TargoneSpikeTest
ERRO: O sistema não pode encontrar o arquivo especificado.             [gone] <!-- codespell:ignore erro -->
> Get-ScheduledTask | ? TaskName -like "TargoneSpike*"
(count: 0)
```

`TargoneSpikeIdle` and `TargoneSpikeTestPS` were likewise deleted at their
steps (exit 0 each). The machine ends the spike with **zero** spike tasks.

## Linux / macOS (research + WSL observation)

### Verified locally (WSL Ubuntu-24.04, systemd 255 man pages + live probe)

- **WSL observation:** `ps -p 1 -o comm=` → `init(Ubuntu-24.04)` — PID 1 is
  WSL's init, not systemd; `/etc/wsl.conf` has no `[boot] systemd=true`.
  Consequently `systemctl --user is-system-running` → `offline` and
  `Failed to connect to bus: No medium found`. `crontab` binary exists but
  `service cron status` → "cron is not running". Empirical takeaway: on a
  default-configured WSL distro **neither** systemd user timers **nor** cron
  are live — the opportunistic fallback is not hypothetical.
- **systemd.timer(5)** (local man page, systemd 255): `OnCalendar=` calendar
  timers; `Persistent=true` — "the service unit is triggered immediately if
  it would have been triggered at least once during the time when the timer
  was inactive ... only has an effect on timers configured with
  OnCalendar=. Defaults to false"; `RandomizedDelaySec=` random 0..N spread;
  `WakeSystem=` (system-resume, needs privileges — not for Targone);
  `systemctl clean --what=state` removes the Persistent stamp before
  uninstall.
- **systemd.unit(5)** (local): user unit search path includes
  `~/.config/systemd/user/*`; `ConditionACPower=` — "If set to 'true', the
  condition will hold only if at least one AC connector of the system is
  connected to a power source, or if no AC connectors are known" (the
  no-battery-desktop case is handled sanely).
- **systemd.exec(5)** (local): `Nice=`, `CPUSchedulingPolicy=` (accepts
  `idle`), `IOSchedulingClass=` (accepts `idle`) — the idle-politeness
  levers, since no timer-level user-idle condition exists.
- **loginctl(1)** (local): `enable-linger` — "a user manager is spawned for
  the user at boot and kept around after logouts. This allows users who are
  not logged in to run long-running services."

### Cited only (macOS — no machine; claims limited to quoted docs)

- **Apple, "Creating Launchd Jobs"** (developer.apple.com archive): per-user
  agents live in `~/Library/LaunchAgents`, "must be owned by that user",
  mode 600/400, and execute "only while that user is logged in".
- **launchd.plist(5)** (Xcode man page): `StartCalendarInterval` — "Unlike
  cron which skips job invocations when the computer is asleep, launchd
  will start the job the next time the computer wakes up"; multiple missed
  intervals are "coalesced into one event upon wake from sleep".
  `ProcessType` (`Background` = resource-limited so as not to disrupt the
  user), `Nice`, `LowPriorityIO`. **No key gates on battery/AC or
  user-idle state** — documented absence.
- **launchctl(1)** (Xcode man page): `bootstrap`/`bootout` with
  domain-target `gui/<uid>` are the modern load/unload; `load`/`unload`
  are listed under "LEGACY SUBCOMMANDS"; `kickstart` runs a service
  "immediately, regardless of its configured launch conditions"; root is
  required for *system*-domain modifications (no such statement for the
  user's own gui domain).
- **Unclaimed:** behavior across full power-off (vs sleep), exact
  double-bootstrap error shape, and any TCC/Full-Disk-Access interaction
  with sweeping `~/...` from a background agent — all need a real Mac.

## Method & caveats

- Single Windows 10 Pro machine, default local policy, non-elevated,
  pt-BR locale (outputs quoted verbatim with translations). Group-policy
  variants (task creation restricted, service disabled) untested.
- The S4U denial is one machine's data point; Microsoft's documented model
  (batch logon rights) is consistent with it, but "always needs elevation"
  was not proven in general.
- `RunOnlyIfIdle` semantics (what Windows counts as "idle", 4-minute
  granularity quirks) were not behaviorally tested — only that registration
  encodes it. A follow-up run-observation would firm this up.
- WSL is *not* representative of a native Linux desktop: on a systemd
  distro with a graphical login, `systemctl --user` works out of the box.
  The WSL probe validates the detection/fallback path, not the happy path;
  the happy-path recipe rests on man-page semantics (high confidence, but
  not executed end-to-end here).
- macOS section is intentionally thinner than its implementation will need
  to be; every claim is tied to a quoted source and gaps are flagged
  rather than filled.
