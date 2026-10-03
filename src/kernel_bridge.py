# binvim's bridge to a Jupyter kernel, run by the interpreter the kernel
# runs in (any Python with ipykernel has jupyter_client, which speaks the
# kernel's ZeroMQ protocol). binvim writes one JSON command per line on
# stdin and reads one JSON event per line from stdout. EOF on stdin --
# binvim gone, however it went -- shuts the kernel down.
#
# Outputs are sent in nbformat's on-disk shape (multi-line strings split
# into lists of lines the way nbformat's writer splits them), so binvim
# stores them as they come.

import sys

# Run from binvim's own directory with -c, so '' on sys.path is not the
# notebook's directory, but drop it anyway: a json.py or queue.py there
# must not stand in for the standard library here. The kernel itself
# still starts in the notebook's directory, as Jupyter starts it.
sys.path[:] = [p for p in sys.path if p]

import json
import queue
import threading
import traceback

_SPLIT_MIMES = {"application/javascript", "image/svg+xml"}


def emit(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def split_bundle(data):
    out = {}
    for key, value in data.items():
        if isinstance(value, str) and (key.startswith("text/") or key in _SPLIT_MIMES):
            value = value.splitlines(True)
        out[key] = value
    return out


def to_output(msg_type, content):
    if msg_type == "stream":
        return {
            "output_type": "stream",
            "name": content.get("name", "stdout"),
            "text": content.get("text", "").splitlines(True),
        }
    if msg_type == "execute_result":
        return {
            "output_type": "execute_result",
            "execution_count": content.get("execution_count"),
            "data": split_bundle(content.get("data", {})),
            "metadata": content.get("metadata", {}),
        }
    if msg_type == "display_data":
        return {
            "output_type": "display_data",
            "data": split_bundle(content.get("data", {})),
            "metadata": content.get("metadata", {}),
        }
    if msg_type == "error":
        return {
            "output_type": "error",
            "ename": content.get("ename", ""),
            "evalue": content.get("evalue", ""),
            "traceback": content.get("traceback", []),
        }
    return None


def main():
    cwd = sys.argv[1]
    try:
        from jupyter_client import KernelManager
        from jupyter_client.kernelspec import KernelSpec, KernelSpecManager
    except Exception as e:
        emit({"ev": "dead", "msg": "jupyter_client is not importable: %s" % e})
        return

    # The kernel is this interpreter, whatever kernelspecs say: the
    # interpreter was chosen for the notebook, and a `python3` spec in
    # ~/Library/Jupyter could point anywhere.
    class Spec(KernelSpecManager):
        def get_kernel_spec(self, kernel_name):
            return KernelSpec(
                argv=[sys.executable, "-m", "ipykernel_launcher", "-f", "{connection_file}"],
                display_name="Python",
                language="python",
            )

    cmds = queue.Queue()

    def read_stdin():
        for line in sys.stdin:
            try:
                cmds.put(json.loads(line))
            except ValueError:
                pass
        cmds.put({"op": "shutdown"})

    threading.Thread(target=read_stdin, daemon=True).start()

    import subprocess
    import tempfile

    err = tempfile.TemporaryFile()
    km = KernelManager(kernel_name="python3", kernel_spec_manager=Spec())
    try:
        km.start_kernel(cwd=cwd, stdout=subprocess.DEVNULL, stderr=err)
        kc = km.client()
        kc.start_channels()
        kc.wait_for_ready(timeout=60)
    except Exception as e:
        emit({"ev": "dead", "msg": "the kernel did not start: %s%s" % (e, tail(err))})
        try:
            km.shutdown_kernel(now=True)
        except Exception:
            pass
        return

    emit({"ev": "ready", "python": sys.executable, "version": sys.version.split()[0]})
    pending = {}
    try:
        loop(km, kc, cmds, pending, err)
    finally:
        try:
            kc.stop_channels()
            km.shutdown_kernel(now=True)
        except Exception:
            pass


def tail(err):
    try:
        err.seek(0)
        lines = err.read().decode("utf-8", "replace").strip().splitlines()
    except Exception:
        return ""
    return (": " + lines[-1]) if lines else ""


def loop(km, kc, cmds, pending, err):
    while True:
        while True:
            try:
                cmd = cmds.get_nowait()
            except queue.Empty:
                break
            op = cmd.get("op")
            if op == "execute":
                msg_id = kc.execute(
                    cmd.get("code", ""), store_history=True, allow_stdin=False, stop_on_error=True
                )
                pending[msg_id] = cmd.get("cell")
            elif op == "interrupt":
                km.interrupt_kernel()
            elif op == "shutdown":
                return
        try:
            msg = kc.get_iopub_msg(timeout=0.05)
        except queue.Empty:
            msg = None
        while msg is not None:
            iopub(msg, pending)
            try:
                msg = kc.get_iopub_msg(timeout=0)
            except queue.Empty:
                msg = None
        while kc.shell_channel.msg_ready():
            reply = kc.get_shell_msg(timeout=0)
            parent = reply.get("parent_header", {}).get("msg_id")
            # An execution aborted after an earlier error may never go busy
            # and idle on iopub.
            if reply.get("content", {}).get("status") == "aborted" and parent in pending:
                emit({"ev": "done", "cell": pending.pop(parent), "count": None})
        if not km.is_alive():
            emit({"ev": "dead", "msg": "the kernel died%s" % tail(err)})
            return


def iopub(msg, pending):
    parent = msg.get("parent_header", {}).get("msg_id")
    if parent not in pending:
        return
    cell = pending[parent]
    msg_type = msg.get("msg_type") or msg.get("header", {}).get("msg_type")
    content = msg.get("content", {})
    if msg_type == "status":
        state = content.get("execution_state")
        if state == "busy":
            emit({"ev": "busy", "cell": cell})
        elif state == "idle":
            pending.pop(parent, None)
            emit({"ev": "done", "cell": cell})
    elif msg_type == "execute_input":
        emit({"ev": "count", "cell": cell, "count": content.get("execution_count")})
    elif msg_type == "clear_output":
        emit({"ev": "clear", "cell": cell, "wait": bool(content.get("wait"))})
    else:
        out = to_output(msg_type, content)
        if out is not None:
            emit({"ev": "output", "cell": cell, "output": out})


if __name__ == "__main__":
    try:
        main()
    except Exception:
        emit({"ev": "dead", "msg": traceback.format_exc().strip().splitlines()[-1]})
