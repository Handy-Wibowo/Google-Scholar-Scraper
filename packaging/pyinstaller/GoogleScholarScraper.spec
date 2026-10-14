# -*- mode: python ; coding: utf-8 -*-

from pathlib import Path


project_root = Path(SPECPATH).parents[1]
src_path = project_root / "src"
entrypoint = src_path / "google_scholar_scraper" / "__main__.py"

# Python 3.14 ships Tcl/Tk 9, whose script libraries are embedded in tcl90.dll /
# tcl9tk90.dll behind Tcl's virtual zipfs filesystem. PyInstaller's stock
# pyi_rth__tkinter hook requires external _tcl_data/_tk_data directories that do
# not exist in that layout, and aborts the frozen app on startup. Replace it with
# a hook that only sets TCL_LIBRARY/TK_LIBRARY when such directories were
# actually collected.
runtime_hooks = [str(Path(SPECPATH) / "pyi_rth_tkinter_embedded.py")]
excluded_optional_modules = [
    "aiohttp",
    "cryptography",
    "cv2",
    "Cython",
    "fastapi",
    "flask",
    "h2",
    "hpack",
    "hyperframe",
    "lxml",
    "matplotlib",
    "mlflow",
    "nltk",
    "numpy",
    "onnxruntime",
    "openai",
    "pandas",
    "PIL",
    "psutil",
    "pyarrow",
    "PyQt6",
    "pytest",
    "scipy",
    "sklearn",
    "sqlalchemy",
    "torch",
    "torchvision",
    "transformers",
    "uvicorn",
    "win32com",
]


a = Analysis(
    [str(entrypoint)],
    pathex=[str(src_path)],
    binaries=[],
    datas=[],
    hiddenimports=[],
    hookspath=[],
    hooksconfig={},
    runtime_hooks=runtime_hooks,
    excludes=["tests", *excluded_optional_modules],
    noarchive=False,
)
# Runtime hooks live in Analysis.scripts, not its data or import tables.
a.scripts = [entry for entry in a.scripts if entry[0] != "pyi_rth__tkinter"]
pyz = PYZ(a.pure)

exe = EXE(
    pyz,
    a.scripts,
    [],
    exclude_binaries=True,
    name="GoogleScholarScraper",
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=True,
    console=False,
    disable_windowed_traceback=False,
    argv_emulation=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
)
coll = COLLECT(
    exe,
    a.binaries,
    a.datas,
    strip=False,
    upx=True,
    upx_exclude=[],
    name="Google-Scholar-Scraper-v2.1.0",
)
