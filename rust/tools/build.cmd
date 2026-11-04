@echo off
:: Build driver for the rust workspace on this workstation.
:: windres's inner `cmd /c` preprocessor needs: (1) CONDA_SHLVL defined so
:: the cmd AutoRun conda hook exits silently, (2) mingw64 bin with
:: backslash paths FIRST in PATH so windres resolves gcc for popen.
setlocal
set "CONDA_SHLVL=1"
set "PATH=C:\Users\NITRO 5\AppData\Local\Temp\mingw64\bin;C:\Users\NITRO 5\.cargo\bin;C:\Windows\System32;C:\Windows;C:\Windows\System32\Wbem;C:\Program Files\Git\cmd;C:\ProgramData\scoop\shims;C:\Python314;C:\Python314\Scripts"
"C:\Users\NITRO 5\.cargo\bin\cargo.exe" %*
