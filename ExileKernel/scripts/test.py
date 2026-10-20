#!/usr/bin/env python3
import os, pathlib, subprocess
r=pathlib.Path(__file__).resolve().parents[1]
b=r/'build'; b.mkdir(exist_ok=True)
subprocess.run([os.environ.get('EUK_HOST_RUSTC','rustc'),'--edition=2024','--test',r/'src/memory.rs','-o',b/'memory-tests'],check=True)
subprocess.run([b/'memory-tests'],check=True)
