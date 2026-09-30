import sys
sys.argv = ["x"]
HERE = __file__.rsplit("/", 1)[0]
src = open(HERE + "/param_model.py").read().split("tot = []")[0].replace("__file__", repr(HERE + "/param_model.py"))
exec(src)
src2 = open(HERE + "/spline_test.py").read().split("for k in")[0].replace("__file__", repr(HERE + "/spline_test.py"))
b01 = base.copy()
exec(src2)
base = b01
c = SETS["param_user_point"][0]
c = {k: float(v) for k, v in c.items() if k.startswith("Parametric")}
pts = [(0, 14), (44, 46), (106, 110), (255, 252)]
f = natural([p[0] for p in pts], [p[1] for p in pts])
o = RES["param_user_point"] * 255
a = f(curve(base, c) * 255)
b = curve(f(base * 255) / 255, c) * 255
m = base > 0.01
print("point(param) %.2f" % abs(a - o)[m].mean(), "param(point) %.2f" % abs(b - o)[m].mean())
