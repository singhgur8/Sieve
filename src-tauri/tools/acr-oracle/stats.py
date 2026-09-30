import re, glob, collections
root = "/Users/gurjotsingh/Pictures/Jasmit Natalie Proposal"
vals = collections.defaultdict(list)
for x in glob.glob(root + "/**/*.xmp", recursive=True):
    s = open(x).read()
    head = s.split("<crs:Look>")[0]
    for k, v in re.findall(r'crs:(\w+)="([-+0-9.]+)"', head):
        try:
            vals[k].append(float(v))
        except ValueError:
            pass
for k in sorted(vals):
    v = vals[k]
    nz = [a for a in v if a != 0]
    if len(set(v)) > 1 or (v and v[0] != 0):
        c = collections.Counter(v).most_common(3)
        print("%-34s n=%3d nonzero=%3d min=%8.2f max=%8.2f top=%s" % (k, len(v), len(nz), min(v), max(v), c))
