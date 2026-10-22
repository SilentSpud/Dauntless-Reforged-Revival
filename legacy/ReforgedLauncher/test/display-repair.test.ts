import { test } from 'node:test';
import assert from 'node:assert/strict';
import { repairDisplaySettings } from '../src/main/display-repair';
test('FuturePoet oversized viewport recovers to windowed 720p and preserves unrelated preferences', () => {
  const lines=['[/Script/Archon.ArchonGameUserSettings]','ResolutionSizeX=33424','ResolutionSizeY=828','FullscreenMode=1','Volume=0.7','[Other]','ResolutionSizeX=99999'];
  const fixed=repairDisplaySettings(lines);
  assert.equal(fixed.repaired,true);
  assert.ok(fixed.lines.includes('ResolutionSizeX=1280'));
  assert.ok(fixed.lines.includes('FullscreenMode=2'));
  assert.ok(fixed.lines.includes('Volume=0.7'));
  assert.ok(fixed.lines.includes('ResolutionSizeX=99999'));
  assert.equal(repairDisplaySettings(fixed.lines).repaired,false);
});
test('valid widescreen preferences are unchanged', () => {
  const lines=['[/Script/Engine.GameUserSettings]','ResolutionSizeX=5120','ResolutionSizeY=1440'];
  assert.deepEqual(repairDisplaySettings(lines),{lines,repaired:false});
});

test('Poet fix overrides valid or missing saved settings and stays idempotent',()=>{
  for(const lines of [[],['[/Script/Archon.ArchonGameUserSettings]','ResolutionSizeX=1920','ResolutionSizeY=1080','FullscreenMode=1','WindowPosX=33423','Volume=0.7']]) {
    const fixed=repairDisplaySettings(lines,true);
    assert.equal(fixed.repaired,true);
    assert.ok(fixed.lines.includes('ResolutionSizeX=1280'));
    assert.ok(fixed.lines.includes('FullscreenMode=2'));
    assert.ok(fixed.lines.includes('WindowPosX=-1'));
    if(lines.includes('Volume=0.7'))assert.ok(fixed.lines.includes('Volume=0.7'));
    assert.deepEqual(repairDisplaySettings(fixed.lines,true),{lines:fixed.lines,repaired:false});
  }
});
