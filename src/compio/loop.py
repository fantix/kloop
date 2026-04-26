# SPDX-License-Identifier: Apache-2.0 OR MulanPSL-2.0
# Copyright 2025 Fantix King

from __future__ import annotations
from collections.abc import Callable
from typing import Any, Optional, TypeAlias

import asyncio
import enum
import traceback
from asyncio.log import logger

from compio import _core


_Context: TypeAlias = dict[str, Any]
_ExceptionHandler: TypeAlias = Callable[[asyncio.AbstractEventLoop, _Context], object]


class DriverType(enum.StrEnum):
    IO_URING = "IoUring"
    POLL = "Poll"
    IOCP = "IOCP"


class KtlsMode(enum.StrEnum):
    DEFAULT = "Default"
    PREFER = "Prefer"
    REQUIRE = "Require"
    DISABLED = "Disabled"


class CompioLoop(_core.CompioLoop, asyncio.AbstractEventLoop):
    _exception_handler: Optional[_ExceptionHandler]

    __slots__ = ("_exception_handler",)

    def __init__(self) -> None:
        super().__init__()
        self._exception_handler = None

    def get_driver_type(self) -> DriverType:
        return DriverType(super().get_driver_type())

    @property  # type: ignore[override]
    def ktls_mode(self) -> KtlsMode:
        return KtlsMode(super().ktls_mode)

    @ktls_mode.setter
    def ktls_mode(self, mode: KtlsMode) -> None:
        _core.CompioLoop.ktls_mode.__set__(self, mode.value)  # type: ignore[attr-defined]

    def __repr__(self) -> str:
        try:
            driver_type = f"driver={self.get_driver_type().value} "
        except RuntimeError:
            driver_type = ""
        return (
            f"<{self.__class__.__name__} "
            f"{driver_type}"
            f"running={self.is_running()} "
            f"closed={self.is_closed()}>"
        )

    # Exception handling methods copied from CPython 3.13

    def get_exception_handler(self) -> Optional[_ExceptionHandler]:
        """Return an exception handler, or None if the default one is in use."""
        return self._exception_handler

    def set_exception_handler(self, handler: Optional[_ExceptionHandler]) -> None:
        """Set handler as the new event loop exception handler.

        If handler is None, the default exception handler will
        be set.

        If handler is a callable object, it should have a
        signature matching '(loop, context)', where 'loop'
        will be a reference to the active event loop, 'context'
        will be a dict object (see `call_exception_handler()`
        documentation for details about context).
        """
        if handler is not None and not callable(handler):
            raise TypeError(f"A callable object or None is expected, got {handler!r}")
        self._exception_handler = handler

    def default_exception_handler(self, context: _Context) -> None:
        """Default exception handler.

        This is called when an exception occurs and no exception
        handler is set, and can be called by a custom exception
        handler that wants to defer to the default behavior.

        This default handler logs the error message and other
        context-dependent information.  In debug mode, a truncated
        stack trace is also appended showing where the given object
        (e.g. a handle or future or task) was created, if any.

        The context parameter has the same meaning as in
        `call_exception_handler()`.
        """
        message = context.get("message")
        if not message:
            message = "Unhandled exception in event loop"

        exception = context.get("exception")
        exc_info: Any
        if exception is not None:
            exc_info = (type(exception), exception, exception.__traceback__)
        else:
            exc_info = False

        log_lines = [message]
        for key in sorted(context):
            if key in {"message", "exception"}:
                continue
            value = context[key]
            if key == "source_traceback":
                tb = "".join(traceback.format_list(value))
                value = "Object created at (most recent call last):\n"
                value += tb.rstrip()
            elif key == "handle_traceback":
                tb = "".join(traceback.format_list(value))
                value = "Handle created at (most recent call last):\n"
                value += tb.rstrip()
            else:
                value = repr(value)
            log_lines.append(f"{key}: {value}")

        logger.error("\n".join(log_lines), exc_info=exc_info)

    def call_exception_handler(self, context: _Context) -> None:
        """Call the current event loop's exception handler.

        The context argument is a dict containing the following keys:

        - 'message': Error message;
        - 'exception' (optional): Exception object;
        - 'future' (optional): Future instance;
        - 'task' (optional): Task instance;
        - 'handle' (optional): Handle instance;
        - 'protocol' (optional): Protocol instance;
        - 'transport' (optional): Transport instance;
        - 'socket' (optional): Socket instance;
        - 'source_traceback' (optional): Traceback of the source;
        - 'handle_traceback' (optional): Traceback of the handle;
        - 'asyncgen' (optional): Asynchronous generator that caused
                                 the exception.

        New keys maybe introduced in the future.

        Note: do not overload this method in an event loop subclass.
        For custom exception handling, use the
        `set_exception_handler()` method.
        """
        if self._exception_handler is None:
            try:
                self.default_exception_handler(context)
            except (SystemExit, KeyboardInterrupt):
                raise
            except BaseException:
                # Second protection layer for unexpected errors
                # in the default implementation, as well as for subclassed
                # event loops with overloaded "default_exception_handler".
                logger.error("Exception in default exception handler", exc_info=True)
        else:
            try:
                ctx = None
                thing = context.get("task")
                if thing is None:
                    # Even though Futures don't have a context,
                    # Task is a subclass of Future,
                    # and sometimes the 'future' key holds a Task.
                    thing = context.get("future")
                if thing is None:
                    # Handles also have a context.
                    thing = context.get("handle")
                if thing is not None and hasattr(thing, "get_context"):
                    ctx = thing.get_context()
                if ctx is not None and hasattr(ctx, "run"):
                    ctx.run(self._exception_handler, self, context)
                else:
                    self._exception_handler(self, context)
            except (SystemExit, KeyboardInterrupt):
                raise
            except BaseException as exc:
                # Exception in the user set custom exception handler.
                try:
                    # Let's try default handler.
                    self.default_exception_handler(
                        {
                            "message": "Unhandled error in exception handler",
                            "exception": exc,
                            "context": context,
                        }
                    )
                except (SystemExit, KeyboardInterrupt):
                    raise
                except BaseException:
                    # Guard 'default_exception_handler' in case it is
                    # overloaded.
                    logger.error(
                        "Exception in default exception handler "
                        "while handling an unexpected error "
                        "in custom exception handler",
                        exc_info=True,
                    )
