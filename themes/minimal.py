# Minimal Outsider theme
# Single-line prompt with exit-code indicator.

SUCCESS = "#22c55e"
ERROR = "#ef4444"
DIM = "#6b7280"


def render_prompt(**context):
    cwd = context.get("cwd", "~")
    exit_code = int(context.get("exit_code", "0") or "0")
    char = "\u276f" if exit_code == 0 else "\u2718"
    return {
        "lines_above": [],
        "input_prefix": f"{cwd} {char} ",
        "right_prompt": "",
        "colors": {"success": SUCCESS, "error": ERROR, "dim": DIM},
    }