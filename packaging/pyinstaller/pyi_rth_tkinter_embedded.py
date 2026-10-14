"""Use collected Tcl/Tk scripts, or let Tcl/Tk 9 load its embedded zipfs scripts."""

import os
import sys


def _pyi_rthook():
    for env_name, bundled_name in (("TCL_LIBRARY", "_tcl_data"), ("TK_LIBRARY", "_tk_data")):
        bundled_dir = os.path.join(sys._MEIPASS, bundled_name)
        if os.path.isdir(bundled_dir):
            os.environ[env_name] = bundled_dir
        else:
            os.environ.pop(env_name, None)


_pyi_rthook()
del _pyi_rthook
