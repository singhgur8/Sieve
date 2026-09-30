#!/usr/bin/python3
"""mask_variants.py NAME KEEP: Camera Raw renders of the mask-render reference DNGs
(`test-data/mask-render/ref/<stem>.dng`, copies only) with edited settings.

- XMP_FILE=<dir>/STEM.xmp: replace the DNG's XMP with that packet (STEM = frame), e.g. the
  radial-mask variants from `examples/local_variants`. This is the mode that works for masks:
  editing any value of an *AI* mask makes DNG Converter drop the mask (it cannot recompute
  the matte), geometric masks render.
- Local sliders not named in KEEP (comma list of crs:Local* names without 'Local', e.g.
  Temperature,Clarity2012) are set to 0; GLOBAL_EXPO sets crs:Exposure2012.
Output: test-data/mask-polish/var/NAME/<stem>.ref.jpg (+ the rendered .xmp)."""
import os, re, shutil, subprocess, sys, tempfile

T = os.environ.get('SIEVE_TEST_DATA', os.path.join(os.path.dirname(os.path.abspath(__file__)), '../../../test-data'))
REF = T + '/mask-render/ref'
CONV = '/Applications/Adobe DNG Converter.app/Contents/MacOS/Adobe DNG Converter'
SLIDERS = ['Temperature', 'Tint', 'Exposure2012', 'Contrast2012', 'Highlights2012', 'Shadows2012',
           'Whites2012', 'Blacks2012', 'Texture', 'Clarity2012', 'Dehaze', 'Saturation']
name, keep = sys.argv[1], sys.argv[2].split(',')
out = f'{T}/mask-polish/var/{name}'
os.makedirs(out, exist_ok=True)
stems = sorted(f[:-4] for f in os.listdir(REF) if f.endswith('.dng'))
for stem in stems:
    if os.path.exists(f'{out}/{stem}.ref.jpg'):
        continue
    tmp = tempfile.mkdtemp(dir=T + '/mask-polish')
    os.makedirs(tmp + '/in'); os.makedirs(tmp + '/out')
    dng = f'{tmp}/in/{stem}.dng'
    shutil.copy(f'{REF}/{stem}.dng', dng)
    xmp = subprocess.run(['exiftool', '-m', '-xmp', '-b', dng], capture_output=True, text=True).stdout
    if os.environ.get("GLOBAL_EXPO"):
        xmp = re.sub(r'crs:Exposure2012="[^"]*"', f'crs:Exposure2012="{os.environ["GLOBAL_EXPO"]}"', xmp)
    if os.environ.get("XMP_FILE"):
        xmp = open(os.environ["XMP_FILE"].replace("STEM", stem)).read()
    for s in SLIDERS:
        if s not in keep:
            xmp = re.sub(rf'crs:Local{s}="[^"]*"', f'crs:Local{s}="0"', xmp)
    with open(f'{tmp}/x.xmp', 'w') as f:
        f.write(xmp)
    subprocess.run(['exiftool', '-m', '-q', '-overwrite_original', f'-xmp<={tmp}/x.xmp', dng], check=True)
    subprocess.run([CONV, '-c', '-p2', '-d', tmp + '/out', dng], capture_output=True)
    jpg = subprocess.run(['exiftool', '-m', '-b', '-JpgFromRaw', f'{tmp}/out/{stem}.dng'], capture_output=True).stdout
    with open(f'{out}/{stem}.ref.jpg', 'wb') as f:
        f.write(jpg)
    shutil.copy(f'{tmp}/x.xmp', f'{out}/{stem}.xmp')
    shutil.rmtree(tmp)
    print(name, stem, len(jpg), flush=True)
