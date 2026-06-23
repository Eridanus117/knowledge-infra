"""memex"""

import warnings

# jieba 0.42 内置正则在 py3.13+ 触发 SyntaxWarning(库自身 bug);只抑制 jieba 的,
# 不全局吞 warning。须在任何 jieba import 前(本包入口最先加载)。
warnings.filterwarnings(
    "ignore", message="invalid escape sequence", category=SyntaxWarning
)

__version__ = "0.1.3"
