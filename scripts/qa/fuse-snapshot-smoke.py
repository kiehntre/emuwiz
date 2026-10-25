#!/usr/bin/env python3
"""Original CC0-1.0 fixture and disposable driver; requires Xvfb, ImageMagick, xdotool."""
import os, sys, pathlib, hashlib, json, subprocess, tempfile, time
E = pathlib.Path(tempfile.gettempdir())
R = pathlib.Path(tempfile.mkdtemp(prefix='emuwiz-job20-runtime-'))
print('Evidence:', R, flush=True)
for name in ['content','home','config/retroarch','data','cache','runtime','tmp','saves','states','logs','other']:
    (R/name).mkdir(parents=True,exist_ok=True)
os.chmod(R/'runtime',0o700)
b = bytearray(30+49152)
b[6:8] = (32768).to_bytes(2,'little'); b[8:10] = (65534).to_bytes(2,'little')
# DI. Loop: border <- (40000)&7, upper attributes <- (40001), lower <- (40002).
code = bytearray([0xf3,0x3a,0x40,0x9c,0xe6,7,0xd3,0xfe])
for var, dest in [(40001,22528),(40002,22912)]:
    code += bytes([0x3a,var&255,var>>8,0x21,dest&255,dest>>8,0x77,0x11,(dest+1)&255,(dest+1)>>8,0x01,0x7f,0x01,0xed,0xb0])
code += bytes([0xc3,1,0x80])
b[30+32768-16384:30+32768-16384+len(code)] = code
for addr,value in [(40000,2),(40001,16),(40002,8)]: b[30+addr-16384] = value
p=R/'content/activation.z80'; p.write_bytes(b)
trainer = b'N Green upper and border\nM 8 40000 4 2\nZ 8 40001 32 0\nN Magenta lower (unselected)\nZ 8 40002 40 0\nY\n'
p.with_suffix('.pok').write_bytes(trainer)
sha=lambda b:hashlib.sha256(b).hexdigest()
# The actual Blue smoke seed creates this pin through Gold after its real DAT audit.
(R/'fixture.dat').write_text(f'<datafile><header><name>Original CC0 Job20 fixture</name></header><game name="activation"><rom name="activation.z80" size="{len(b)}" sha256="{sha(b)}"/></game></datafile>\n')
(R/'fixture-license.txt').write_text('The original fixture, trainer, DAT and harness in this evidence directory are dedicated to the public domain under CC0-1.0. No commercial game or external ROM was downloaded.\n')
cfg = f'''libretro_directory = "/usr/lib/x86_64-linux-gnu/libretro"
libretro_info_path = "/usr/share/libretro/info"
video_driver = "gl"
video_fullscreen = false
video_windowed_fullscreen = false
video_window_width = 640
video_window_height = 480
video_shader_enable = false
video_font_enable = false
audio_enable = false
input_driver = "x"
input_joypad_driver = "null"
input_exit_emulator = "escape"
quit_press_twice = false
pause_nonactive = false
config_save_on_exit = false
savestate_auto_load = true
savestate_auto_save = false
log_verbosity = true
libretro_log_level = 0
log_to_file = true
log_to_file_timestamp = true
log_dir = "{R/'logs'}"
savefile_directory = "{R/'saves'}"
savestate_directory = "{R/'states'}"
cheat_database_path = "{R/'empty-cheats'}"
'''
(R/'empty-cheats').mkdir()
(R/'config/retroarch/retroarch.cfg').write_text(cfg)
# Disposable save/state sentinels; no valid auto-state, so each starts from the source snapshot.
(R/'saves/sentinel').write_bytes(b'protected save sentinel\n')
(R/'states/sentinel').write_bytes(b'protected state sentinel\n')
protected = [*sorted((R/'content').iterdir()), R/'config/retroarch/retroarch.cfg',R/'saves/sentinel',R/'states/sentinel']
def manifest(): return {str(p.relative_to(R)):sha(p.read_bytes()) for p in protected}
before=manifest(); (R/'hashes-before.json').write_text(json.dumps(before,indent=2))
env=os.environ.copy()
for key,val in {'HOME':'home','XDG_CONFIG_HOME':'config','XDG_DATA_HOME':'data','XDG_CACHE_HOME':'cache','XDG_RUNTIME_DIR':'runtime','TMPDIR':'tmp','TMP':'tmp','TEMP':'tmp'}.items(): env[key]=str(R/val)
env['LIBGL_ALWAYS_SOFTWARE']='1'
env['PATH']='/usr/games:/usr/bin:/bin'
env['EMUWIZ_JOB20_SMOKE_ROOT']=str(R)
env['EMUWIZ_CONFIG_HOME']=str(R/'app-config')
env['EMUWIZ_DATA_HOME']=str(R/'app-data')
(R/'app-config').mkdir(); (R/'app-data').mkdir()
# Pick a free X display through Xvfb itself, with a bounded startup.
rfd,wfd=os.pipe()
xlog=(R/'xvfb.log').open('wb')
xvfb=subprocess.Popen(['/usr/bin/Xvfb','-displayfd',str(wfd),'-screen','0','800x600x24','-nolisten','tcp'],pass_fds=[wfd],stdout=xlog,stderr=xlog)
os.close(wfd)
import select
if not select.select([rfd],[],[],10)[0]: raise RuntimeError('Xvfb startup timeout')
display=os.read(rfd,100).decode().strip();os.close(rfd)
if not display.isdigit(): raise RuntimeError('Xvfb could not bind a local display; see xvfb.log')
env['DISPLAY']=':'+display
binary=sys.argv[1]
try:
    with (R/'blue-smoke.log').open('wb') as log:
        result=subprocess.run([binary,'--exact','gui_v2::retroarch_journey_tests::fuse_snapshot::installed_fuse_activation_through_blue','--ignored','--nocapture','--test-threads=1'],env=env,cwd=R,stdout=log,stderr=log,timeout=150)
    if result.returncode: raise RuntimeError(f'Blue smoke failed: see {R}/blue-smoke.log')
finally:
    xvfb.terminate();xvfb.wait(timeout=10);xlog.close()
from PIL import Image
observed={}
for mode,channel in [('baseline',0),('selected',1),('after-off',0)]:
    im=Image.open(R/f'{mode}.png').convert('RGB')
    pixels={key:im.getpixel(xy) for key,xy in {'border':(20,20),'upper':(400,180),'lower':(400,420)}.items()}
    for key in ['border','upper']:
        assert 160<=pixels[key][channel]<=220 and all(v==0 for i,v in enumerate(pixels[key]) if i!=channel),(mode,pixels)
    assert pixels['lower'][:2]==(0,0) and 160<=pixels['lower'][2]<=220,(mode,pixels)
    observed[mode]=pixels
assert observed['baseline']==observed['after-off']
assert observed['baseline']['lower']==observed['selected']['lower']
receipt=json.loads((R/'selected-receipt.json').read_text())
assert receipt['runtime_effect']=='UNKNOWN'
assert receipt['cleanup']['status']=='completed' and not receipt['errors']
assert receipt['process']['exit']['success']
assert [c['title'] for c in receipt['cheats']]==['Green upper and border']
assert json.loads((R/'blue-hashes-before.json').read_text())==json.loads((R/'blue-hashes-after.json').read_text())
assert manifest()==before
settings=(R/'selected-workspace-copy/retroarch/profile/retroarch.cfg').read_text()
assert 'savestate_auto_load = "false"' in settings
(R/'visual-verification.json').write_text(json.dumps({'result':'CONFIRMED for tested approved backend and original snapshot through Blue normal launch','pixels':observed},indent=2)+'\n')
print('PASS: real activation visually confirmed through Blue; source hashes unchanged. Evidence:',R)
