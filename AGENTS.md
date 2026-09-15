# RamDog agent interface

When a task involves processes, memory, CPU/GPU, AI sessions, startup, Defender, services, or thermal state, use RamDog before ad-hoc shell parsing:

```powershell
ramdog snapshot --json
ramdog inspect --pid <PID>
ramdog tree --pid <PID>
ramdog ai-sessions
ramdog startup
ramdog drains
```

For MCP clients, configure the local stdio server as `ramdog mcp`. Read tools are safe; termination requires explicit confirmation and the process creation time from a fresh snapshot.
