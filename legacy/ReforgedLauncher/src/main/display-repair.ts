// From Dauntless Revived: a saved window size the game cannot use is repaired before launch.
// The shipped D3D11 client cannot allocate a texture wider/taller than 16384.
// Repair only impossible saved dimensions, preserving ordinary display choices.
const dimensions = /^(?:ResolutionSize[XY]|LastUserConfirmedResolutionSize[XY]|DesiredScreen(?:Width|Height)|LastUserConfirmedDesiredScreen(?:Width|Height))$/i;
export function repairDisplaySettings(lines: string[], forceSafeWindow = false): {lines:string[], repaired:boolean} {
  let section = false;
  const bad = lines.some(line => {
    const heading = /^\s*\[([^\]]+)\]/.exec(line);
    if (heading) section = /(?:^|\.)[A-Za-z]*GameUserSettings$/i.test(heading[1]);
    const match = /^\s*([^=]+?)\s*=\s*([^;]*)/.exec(line);
    if (!section || !match || !dimensions.test(match[1])) return false;
    const value = Number(match[2]);
    return !Number.isFinite(value) || value < 1 || value > 16384;
  });
  if (!bad && !forceSafeWindow) return {lines, repaired:false};
  // A clean/missing INI can still inherit a bad desktop/fullscreen size. The opt-in
  // Poet fix writes safe settings even when the old saved-dimension heuristic finds nothing.
  if (!lines.some(line=>/^\s*\[[^\]]*(?:^|\.)[A-Za-z]*GameUserSettings\]/i.test(line))) {
    lines=[...lines,'[/Script/Engine.GameUserSettings]','[/Script/Archon.ArchonGameUserSettings]'];
  }
  section = false;
  const out: string[] = [];
  for (const line of lines) {
    const heading = /^\s*\[([^\]]+)\]/.exec(line);
    if (heading) {
      section = /(?:^|\.)[A-Za-z]*GameUserSettings$/i.test(heading[1]);
      out.push(line);
      if (section) out.push('ResolutionSizeX=1280','ResolutionSizeY=720','LastUserConfirmedResolutionSizeX=1280','LastUserConfirmedResolutionSizeY=720','FullscreenMode=2','LastConfirmedFullscreenMode=2','PreferredFullscreenMode=2','WindowPosX=-1','WindowPosY=-1');
      continue;
    }
    const key = /^\s*([^=]+?)\s*=/.exec(line)?.[1];
    if (section && key && (dimensions.test(key) || /^(FullscreenMode|LastConfirmedFullscreenMode|PreferredFullscreenMode|WindowPos[XY])$/i.test(key))) continue;
    out.push(line);
  }
  return {lines:out,repaired:out.join('\n') !== lines.join('\n')};
}
