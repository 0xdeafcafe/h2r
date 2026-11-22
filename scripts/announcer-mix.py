#!/usr/bin/env python3
"""Match announcer WAVs to the retail announcer mix (art/announcer/README.md).

Usage: python3 -I scripts/announcer-mix.py RETAIL_DIR SOURCE_DIR OUT_DIR
RETAIL_DIR holds retail lines exported from your own disc with `h2 sound`;
nothing from it is written to OUT_DIR but measured EQ and loudness.
"""
import json, subprocess, sys, glob, os, statistics as st
def run(a): return subprocess.run(a, capture_output=True, text=True)
B=[("lo","lowpass=f=200"),("lm","bandpass=f=450:width_type=h:w=500"),("mid","bandpass=f=2000:width_type=h:w=2000"),("hm","bandpass=f=4500:width_type=h:w=3000"),("hi","highpass=f=6000")]
def bands(f):
    o={}
    for n,flt in B:
        r=run(["ffmpeg","-hide_banner","-i",f,"-af",f"{flt},volumedetect","-f","null","-"]).stderr
        o[n]=float([l for l in r.splitlines() if "mean_volume" in l][0].split(":")[1].split()[0])
    return {k:o[k]-o["mid"] for k in o}
def loud(f):
    s=run(["ffmpeg","-hide_banner","-i",f,"-af","loudnorm=print_format=json","-f","null","-"]).stderr
    return json.loads(s[s.rindex("{"):s.rindex("}")+1])
retail=sorted(glob.glob(sys.argv[1]+"/**/*.wav",recursive=True))
rt=[bands(f) for f in retail]; T={k:st.median(r[k] for r in rt) for k in rt[0]}
TI=st.median(float(loud(f)["input_i"]) for f in retail)
src,out=sys.argv[2],sys.argv[3]; os.makedirs(out,exist_ok=True)
def chain(g,vol):
    return ",".join([f"lowshelf=f=200:g={g['lo']:.1f}", f"equalizer=f=450:width_type=o:width=1.2:g={g['lm']:.1f}",
        f"equalizer=f=4500:width_type=o:width=1.0:g={g['hm']:.1f}", f"highshelf=f=6000:g={g['hi']:.1f}",
        "acompressor=threshold=-24dB:ratio=3:attack=5:release=80", f"volume={vol:.2f}dB",
        "alimiter=limit=0.94:attack=2:release=40:level=false"])
for f in sorted(glob.glob(src+"/*.wav")):
    o=out+"/"+os.path.basename(f); g={"lo":0,"lm":0,"hm":0,"hi":0}; vol=6.0
    for it in range(5):
        run(["ffmpeg","-hide_banner","-loglevel","error","-y","-i",f,"-af",chain(g,vol),"-ac","1","-ar","44100","-c:a","pcm_s16le",o])
        b=bands(o); I=float(loud(o)["input_i"])
        for k in g: g[k]=max(-20,min(20,g[k]+(T[k]-b[k])))
        vol+=TI-I
    l=loud(o)
    print(f"{os.path.basename(f):18} I={l['input_i']} TP={l['input_tp']} tilt "+" ".join(f"{k}:{b[k]-T[k]:+.1f}" for k in g))
print("target",{k:round(v,1) for k,v in T.items()},"I",TI)
