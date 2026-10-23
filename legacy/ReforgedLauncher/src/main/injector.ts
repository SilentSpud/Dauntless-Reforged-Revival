// Loads a DLL the host chose (Dumper-7, or the CatalogExporter built from their own SDK) into the
// game process this launcher started, with the classic CreateRemoteThread(LoadLibraryW) call. This
// is how Mystic Paradox's guides say to generate the SDK and the game data ("inject Dumper-7 into
// your own 1.14.7 client"); the launcher only saves the host a separate injector.
//
// It runs as a PowerShell script with a small C# helper (no native module, nothing to build). It
// only ever targets the process id of the game this launcher started, and only a .dll the host
// picked in a file dialog. Players never see this: it lives on the Host page.

import { execFile } from "node:child_process";
import { promises as fsp } from "node:fs";
import os from "node:os";
import path from "node:path";

export const INJECT_SCRIPT = String.raw`param(
  [Parameter(Mandatory = $true)][int]$ProcessId,
  [Parameter(Mandatory = $true)][string]$DllPath,
  [int]$WaitWindowSeconds = 180,
  [int]$ExtraDelaySeconds = 15
)
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class ReforgedInject {
  [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
  [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr VirtualAllocEx(IntPtr h, IntPtr addr, UIntPtr size, uint type, uint protect);
  [DllImport("kernel32.dll", SetLastError = true)] static extern bool VirtualFreeEx(IntPtr h, IntPtr addr, UIntPtr size, uint type);
  [DllImport("kernel32.dll", SetLastError = true)] static extern bool WriteProcessMemory(IntPtr h, IntPtr addr, byte[] buf, UIntPtr size, out UIntPtr written);
  [DllImport("kernel32.dll", CharSet = CharSet.Ansi, SetLastError = true)] static extern IntPtr GetProcAddress(IntPtr module, string name);
  [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] static extern IntPtr GetModuleHandleW(string name);
  [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr CreateRemoteThread(IntPtr h, IntPtr attr, UIntPtr stack, IntPtr start, IntPtr param, uint flags, IntPtr id);
  [DllImport("kernel32.dll")] static extern uint WaitForSingleObject(IntPtr h, uint ms);
  [DllImport("kernel32.dll")] static extern bool GetExitCodeThread(IntPtr h, out uint code);
  [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
  public static string Inject(int pid, string dll) {
    const uint Access = 0x0002 | 0x0400 | 0x0008 | 0x0010 | 0x0020;
    IntPtr process = OpenProcess(Access, false, pid);
    if (process == IntPtr.Zero) return "open_process_failed:" + Marshal.GetLastWin32Error();
    try {
      byte[] bytes = Encoding.Unicode.GetBytes(dll + "\0");
      IntPtr remote = VirtualAllocEx(process, IntPtr.Zero, (UIntPtr)bytes.Length, 0x3000, 0x04);
      if (remote == IntPtr.Zero) return "alloc_failed:" + Marshal.GetLastWin32Error();
      try {
        UIntPtr written;
        if (!WriteProcessMemory(process, remote, bytes, (UIntPtr)bytes.Length, out written)) return "write_failed:" + Marshal.GetLastWin32Error();
        IntPtr loadLibrary = GetProcAddress(GetModuleHandleW("kernel32.dll"), "LoadLibraryW");
        if (loadLibrary == IntPtr.Zero) return "no_loadlibrary";
        IntPtr thread = CreateRemoteThread(process, IntPtr.Zero, UIntPtr.Zero, loadLibrary, remote, 0, IntPtr.Zero);
        if (thread == IntPtr.Zero) return "thread_failed:" + Marshal.GetLastWin32Error();
        try {
          WaitForSingleObject(thread, 60000);
          uint code;
          GetExitCodeThread(thread, out code);
          return code != 0 ? "ok" : "load_failed";
        } finally { CloseHandle(thread); }
      } finally { VirtualFreeEx(process, remote, UIntPtr.Zero, 0x8000); }
    } finally { CloseHandle(process); }
  }
}
"@
function Say($event, $detail) { [Console]::Out.WriteLine((@{ event = $event; detail = $detail } | ConvertTo-Json -Compress)) }
if (-not (Test-Path -LiteralPath $DllPath -PathType Leaf)) { Say 'failed' 'dll_missing'; exit 2 }
Say 'waiting' 'window'
$deadline = (Get-Date).AddSeconds($WaitWindowSeconds)
while ($true) {
  $p = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
  if ($null -eq $p) { Say 'failed' 'process_gone'; exit 3 }
  if ($p.MainWindowHandle -ne 0) { break }
  if ((Get-Date) -gt $deadline) { break }
  Start-Sleep -Milliseconds 500
}
Say 'waiting' 'settle'
Start-Sleep -Seconds $ExtraDelaySeconds
if ($null -eq (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue)) { Say 'failed' 'process_gone'; exit 3 }
$result = [ReforgedInject]::Inject($ProcessId, $DllPath)
if ($result -eq 'ok') { Say 'injected' $DllPath; exit 0 }
Say 'failed' $result
exit 4
`;

export interface InjectEvent {
  event: "waiting" | "injected" | "failed";
  detail: string;
}

export function parseInjectOutput(stdout: string): InjectEvent[] {
  const out: InjectEvent[] = [];
  for (const line of stdout.split(/\r?\n/)) {
    const text = line.trim();
    if (!text.startsWith("{")) continue;
    try {
      const v = JSON.parse(text) as Record<string, unknown>;
      if ((v.event === "waiting" || v.event === "injected" || v.event === "failed") && typeof v.detail === "string") out.push({ event: v.event, detail: v.detail.slice(0, 300) });
    } catch {
      /* not one of ours */
    }
  }
  return out;
}

export interface InjectOptions {
  pid: number;
  dll: string;
  waitWindowSeconds?: number;
  extraDelaySeconds?: number;
  platform?: NodeJS.Platform;
}

// Runs the script and resolves with its last event.
export async function injectDll(opts: InjectOptions): Promise<InjectEvent> {
  if ((opts.platform ?? process.platform) !== "win32") return { event: "failed", detail: "windows_only" };
  if (!Number.isInteger(opts.pid) || opts.pid <= 0) return { event: "failed", detail: "bad_pid" };
  if (!/\.dll$/i.test(opts.dll) || !path.win32.isAbsolute(opts.dll)) return { event: "failed", detail: "bad_dll" };
  const dir = await fsp.mkdtemp(path.join(os.tmpdir(), "reforged-inject-"));
  const script = path.join(dir, "inject.ps1");
  await fsp.writeFile(script, INJECT_SCRIPT, "utf8");
  try {
    const stdout = await new Promise<string>((resolve) => {
      execFile(
        "powershell.exe",
        [
          "-NoProfile",
          "-NonInteractive",
          "-ExecutionPolicy",
          "Bypass",
          "-File",
          script,
          "-ProcessId",
          String(opts.pid),
          "-DllPath",
          opts.dll,
          "-WaitWindowSeconds",
          String(opts.waitWindowSeconds ?? 180),
          "-ExtraDelaySeconds",
          String(opts.extraDelaySeconds ?? 15),
        ],
        { windowsHide: true, timeout: 10 * 60 * 1000, maxBuffer: 1024 * 1024 },
        (_err, out) => resolve(String(out ?? "")),
      );
    });
    const events = parseInjectOutput(stdout);
    return events[events.length - 1] ?? { event: "failed", detail: "no_answer" };
  } finally {
    await fsp.rm(dir, { recursive: true, force: true }).catch(() => undefined);
  }
}
