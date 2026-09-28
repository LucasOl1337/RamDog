# Security policy

RamDog can terminate processes, and some actions run with elevated privileges:

- On Linux, `ramdog --clean-helper` (cache, journal, pacman cleanup) and `ramdog --fan-helper` (PWM control) run as root through `pkexec`, or `sudo -n` when already authorized. The GUI itself never runs as root.
- On Windows, RamDog asks for elevation when it opens, and `hwtemp.exe` reads sensors and drives fans as administrator.
- The installers download release packages and verify them against `SHA256SUMS.txt`.

Issues in these paths (privilege escalation, a helper acting on attacker-controlled paths or arguments, a way to kill protected processes, checksum bypass) are security issues.

## Reporting a vulnerability

Please **do not open a public issue**. Report it privately through GitHub: [Security → Report a vulnerability](https://github.com/LucasOl1337/RamDog/security/advisories/new).

Include the version, the operating system, and the steps to reproduce. You will get an answer within a few days. Fixes ship in a new release, and the advisory credits you unless you prefer otherwise.

## Supported versions

Only the [latest release](https://github.com/LucasOl1337/RamDog/releases/latest) receives security fixes.
