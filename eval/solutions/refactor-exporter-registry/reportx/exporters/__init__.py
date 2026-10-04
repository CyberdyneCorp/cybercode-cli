"""Output format plugins. Every module in this package (except `base` and modules whose name
starts with an underscore) is imported here, so each one's `@register` runs."""

import importlib
import pkgutil

from .base import Exporter, available_formats, get_exporter, register

__all__ = ["Exporter", "available_formats", "get_exporter", "register"]

for _module in pkgutil.iter_modules(__path__):
    if _module.name != "base" and not _module.name.startswith("_"):
        importlib.import_module(f"{__name__}.{_module.name}")
