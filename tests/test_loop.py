# SPDX-License-Identifier: Apache-2.0 OR MulanPSL-2.0
# Copyright 2025 Fantix King

# TODO: drop the following mypy disable when the loop is done
# mypy: disable-error-code="abstract"

import asyncio
import contextvars
import gc
import sys
import unittest

import compio


def has_io_uring_support() -> bool:
    """Check if the system has io_uring support enabled.

    This checks if io_uring is disabled via the kernel.io_uring_disabled sysctl.
    If the sysctl doesn't exist, falls back to checking /proc/kallsyms for io_uring symbols.
    Returns True if io_uring is available (not disabled), False otherwise.
    """
    if sys.platform.startswith("linux"):
        try:
            # Check if kernel.io_uring_disabled is set to 0 (enabled)
            with open("/proc/sys/kernel/io_uring_disabled") as f:
                value = f.read().strip()
                return value == "0"
        except FileNotFoundError:
            # If the sysctl doesn't exist, check /proc/kallsyms as fallback
            # to see if io_uring symbols are present in the kernel
            try:
                with open("/proc/kallsyms") as f:
                    for line in f:
                        # Look for io_uring_setup symbol which is the main entry point
                        if "io_uring_setup" in line:
                            return True
                return False
            except (FileNotFoundError, PermissionError):
                # If we can't read kallsyms, assume io_uring is not available
                return False
        except PermissionError:
            # If we can't read the sysctl due to permissions, assume unavailable
            return False
    else:
        return False


class TestCase(unittest.TestCase):
    def tearDown(self) -> None:
        gc.collect()
        super().tearDown()


class TestCompioLoop(TestCase):
    def test_create_loop(self) -> None:
        """Test that a CompioLoop can be created."""
        loop = compio.CompioLoop()
        self.assertIsInstance(loop, asyncio.AbstractEventLoop)

    def test_driver_type(self) -> None:
        """Test that driver_type property returns a valid DriverType."""
        loop = compio.CompioLoop()
        self.assertIsInstance(loop.get_driver_type(), compio.DriverType)

        # Platform-specific driver type validation
        if sys.platform == "win32":
            # Windows should use IOCP
            self.assertEqual(loop.get_driver_type(), compio.DriverType.IOCP)
        elif has_io_uring_support():
            # Linux with successful io_uring driver initialization
            self.assertEqual(loop.get_driver_type(), compio.DriverType.IO_URING)
        else:
            # Otherwise, should fall back to POLL
            self.assertEqual(loop.get_driver_type(), compio.DriverType.POLL)

    def test_driver_type_in_repr(self) -> None:
        """Test that driver_type appears in the repr string."""
        loop = compio.CompioLoop()
        repr_str = repr(loop)
        self.assertIn("CompioLoop", repr_str)
        self.assertIn(loop.get_driver_type().value, repr_str)

    def test_no_memory_leak(self) -> None:
        """Test that creating and destroying loops doesn't leak Python objects."""

        # Force garbage collection to start with a clean slate
        gc.collect()

        # Get initial object count
        initial_objects = len(gc.get_objects())

        # Create and destroy loops multiple times
        for _ in range(100):
            loop = compio.CompioLoop()
            # Access some properties to ensure full initialization
            _ = loop.get_driver_type()
            _ = repr(loop)
            # Explicitly delete the loop
            del loop

        # Force garbage collection
        gc.collect()

        # Get final object count
        final_objects = len(gc.get_objects())

        # Allow some tolerance for GC internals and Python runtime overhead
        # but the difference should be small (< 50 objects for 100 iterations)
        object_diff = final_objects - initial_objects
        self.assertLess(
            object_diff,
            50,
            f"Possible memory leak: {object_diff} objects leaked after 100 loop creations",
        )


class TestLoopState(TestCase):
    def test_initial_state(self) -> None:
        """Test initial state of a new loop."""
        loop = compio.CompioLoop()
        self.assertFalse(loop.is_running())
        self.assertFalse(loop.is_closed())

    def test_closed_state_after_close(self) -> None:
        """Test that is_closed returns True after close."""
        loop = compio.CompioLoop()
        loop.close()
        self.assertTrue(loop.is_closed())
        self.assertFalse(loop.is_running())

    def test_close_idempotent(self) -> None:
        """Test that close can be called multiple times."""
        loop = compio.CompioLoop()
        loop.close()
        loop.close()  # Should not raise
        self.assertTrue(loop.is_closed())

    def test_operations_on_closed_loop_raise(self) -> None:
        """Test that operations on closed loop raise RuntimeError."""
        loop = compio.CompioLoop()
        loop.close()

        with self.assertRaises(RuntimeError):
            loop.get_driver_type()

        with self.assertRaises(RuntimeError):
            loop.call_soon(lambda: None)


class TestCallSoon(TestCase):
    def test_call_soon_returns_handle(self) -> None:
        """Test that call_soon returns a Handle."""
        loop = compio.CompioLoop()
        handle = loop.call_soon(lambda: None)
        self.assertIsInstance(handle, compio.Handle)
        loop.close()

    def test_call_soon_with_args(self) -> None:
        """Test call_soon with arguments."""
        loop = compio.CompioLoop()
        results: list[tuple[int, int, int]] = []

        def callback(a: int, b: int, c: int) -> None:
            results.append((a, b, c))

        loop.call_soon(callback, 1, 2, 3)
        loop.call_soon(loop.stop)
        loop.run_forever()

        self.assertEqual(results, [(1, 2, 3)])
        loop.close()

    def test_call_soon_execution_order(self) -> None:
        """Test that callbacks are executed in FIFO order."""
        loop = compio.CompioLoop()
        results: list[int] = []

        for i in range(5):
            loop.call_soon(results.append, i)
        loop.call_soon(loop.stop)
        loop.run_forever()

        self.assertEqual(results, [0, 1, 2, 3, 4])
        loop.close()


class TestHandle(TestCase):
    def test_handle_cancel(self) -> None:
        """Test that cancelled handle's callback is not executed."""
        loop = compio.CompioLoop()
        results: list[str] = []

        handle = loop.call_soon(results.append, "cancelled")
        loop.call_soon(results.append, "executed")
        handle.cancel()
        loop.call_soon(loop.stop)
        loop.run_forever()

        self.assertEqual(results, ["executed"])
        loop.close()

    def test_handle_cancelled_state(self) -> None:
        """Test handle.cancelled() returns correct state."""
        loop = compio.CompioLoop()
        handle = loop.call_soon(lambda: None)

        self.assertFalse(handle.cancelled())
        handle.cancel()
        self.assertTrue(handle.cancelled())
        loop.close()

    def test_handle_cancel_idempotent(self) -> None:
        """Test that cancel can be called multiple times."""
        loop = compio.CompioLoop()
        handle = loop.call_soon(lambda: None)

        handle.cancel()
        handle.cancel()  # Should not raise
        self.assertTrue(handle.cancelled())
        loop.close()


class TestRunForever(TestCase):
    def test_run_forever_and_stop(self) -> None:
        """Test run_forever stops when stop is called."""
        loop = compio.CompioLoop()
        results: list[int] = []

        loop.call_soon(results.append, 1)
        loop.call_soon(results.append, 2)
        loop.call_soon(loop.stop)
        loop.call_soon(results.append, 3)

        loop.run_forever()

        # All callbacks scheduled before run_forever are executed in the same iteration,
        # stop() sets the flag which is checked after processing the ready queue
        self.assertEqual(results, [1, 2, 3])
        loop.close()

    def test_is_running_during_run(self) -> None:
        """Test that is_running returns True during run_forever."""
        loop = compio.CompioLoop()
        running_state: list[bool] = []

        def check_running() -> None:
            running_state.append(loop.is_running())
            loop.stop()

        loop.call_soon(check_running)
        loop.run_forever()

        self.assertEqual(running_state, [True])
        self.assertFalse(loop.is_running())
        loop.close()

    def test_run_forever_already_running(self) -> None:
        """Test that running loop twice raises RuntimeError."""
        loop = compio.CompioLoop()
        error_raised: list[str] = []

        def try_run_again() -> None:
            try:
                loop.run_forever()
            except RuntimeError as e:
                error_raised.append(str(e))
            loop.stop()

        loop.call_soon(try_run_again)
        loop.run_forever()

        self.assertEqual(len(error_raised), 1)
        self.assertIn("already running", error_raised[0])
        loop.close()

    def test_stop_before_run(self) -> None:
        """Test that stop before run_forever causes immediate return."""
        loop = compio.CompioLoop()
        results: list[int] = []

        loop.call_soon(results.append, 1)
        loop.stop()
        loop.run_forever()

        # Callbacks should still be processed until stop is checked
        self.assertEqual(results, [1])
        loop.close()

    def test_run_forever_can_restart(self) -> None:
        """Test that run_forever can be called again after stopping."""
        loop = compio.CompioLoop()
        results: list[str] = []

        loop.call_soon(results.append, "first")
        loop.call_soon(loop.stop)
        loop.run_forever()

        loop.call_soon(results.append, "second")
        loop.call_soon(loop.stop)
        loop.run_forever()

        self.assertEqual(results, ["first", "second"])
        loop.close()


class TestContextVars(TestCase):
    def test_call_soon_copies_context(self) -> None:
        """Test that call_soon copies current context."""
        loop = compio.CompioLoop()
        var: contextvars.ContextVar[str] = contextvars.ContextVar("test_var")
        results: list[str] = []

        var.set("original")

        def callback() -> None:
            results.append(var.get())

        loop.call_soon(callback)

        # Change value after scheduling
        var.set("changed")

        loop.call_soon(loop.stop)
        loop.run_forever()

        # Callback should see "original" since context was copied at schedule time
        self.assertEqual(results, ["original"])
        loop.close()

    def test_call_soon_with_explicit_context(self) -> None:
        """Test call_soon with explicit context argument."""
        loop = compio.CompioLoop()
        var: contextvars.ContextVar[str] = contextvars.ContextVar("test_var")
        results: list[str] = []

        def callback() -> None:
            results.append(var.get())

        # Create a context with a specific value
        var.set("in_context")
        ctx = contextvars.copy_context()

        # Schedule with explicit context
        loop.call_soon(callback, context=ctx)
        loop.call_soon(loop.stop)
        loop.run_forever()

        self.assertEqual(results, ["in_context"])
        loop.close()


class TestClose(TestCase):
    def test_cannot_close_running_loop(self) -> None:
        """Test that closing a running loop raises RuntimeError."""
        loop = compio.CompioLoop()
        error_raised: list[str] = []

        def try_close() -> None:
            try:
                loop.close()
            except RuntimeError as e:
                error_raised.append(str(e))
            loop.stop()

        loop.call_soon(try_close)
        loop.run_forever()

        self.assertEqual(len(error_raised), 1)
        self.assertIn("running", error_raised[0].lower())
        loop.close()


class TestRepr(TestCase):
    def test_repr_shows_running_state(self) -> None:
        """Test that repr shows running state correctly."""
        loop = compio.CompioLoop()

        repr_before = repr(loop)
        self.assertIn("running=False", repr_before)

        running_repr: list[str] = []

        def capture_repr() -> None:
            running_repr.append(repr(loop))
            loop.stop()

        loop.call_soon(capture_repr)
        loop.run_forever()

        self.assertIn("running=True", running_repr[0])
        loop.close()

    def test_repr_shows_closed_state(self) -> None:
        """Test that repr shows closed state correctly."""
        loop = compio.CompioLoop()

        repr_before = repr(loop)
        self.assertIn("closed=False", repr_before)

        loop.close()
        repr_after = repr(loop)
        self.assertIn("closed=True", repr_after)


class TestTime(TestCase):
    def test_time_returns_float(self) -> None:
        """Test that time() returns a float."""
        loop = compio.CompioLoop()
        t = loop.time()
        self.assertIsInstance(t, float)
        self.assertGreaterEqual(t, 0)
        loop.close()

    def test_time_increases(self) -> None:
        """Test that time() increases monotonically."""
        loop = compio.CompioLoop()
        t1 = loop.time()
        t2 = loop.time()
        self.assertGreaterEqual(t2, t1)
        loop.close()


class TestCallLater(TestCase):
    def test_call_later_returns_timer_handle(self) -> None:
        """Test that call_later returns a TimerHandle."""
        loop = compio.CompioLoop()
        handle = loop.call_later(0.1, lambda: None)
        self.assertIsInstance(handle, compio.TimerHandle)
        loop.close()

    def test_call_later_execution(self) -> None:
        """Test that call_later executes callback after delay."""
        loop = compio.CompioLoop()
        results: list[str] = []

        loop.call_later(0.01, results.append, "delayed")
        loop.call_later(0.02, loop.stop)
        loop.run_forever()

        self.assertEqual(results, ["delayed"])
        loop.close()

    def test_call_later_order(self) -> None:
        """Test that call_later callbacks execute in time order."""
        loop = compio.CompioLoop()
        results: list[int] = []

        loop.call_later(0.03, results.append, 3)
        loop.call_later(0.01, results.append, 1)
        loop.call_later(0.02, results.append, 2)
        loop.call_later(0.04, loop.stop)
        loop.run_forever()

        self.assertEqual(results, [1, 2, 3])
        loop.close()

    def test_call_later_with_args(self) -> None:
        """Test call_later with arguments."""
        loop = compio.CompioLoop()
        results: list[tuple[int, int, int]] = []

        def callback(a: int, b: int, c: int) -> None:
            results.append((a, b, c))

        loop.call_later(0.01, callback, 1, 2, 3)
        loop.call_later(0.02, loop.stop)
        loop.run_forever()

        self.assertEqual(results, [(1, 2, 3)])
        loop.close()

    def test_call_later_zero_delay(self) -> None:
        """Test call_later with zero delay."""
        loop = compio.CompioLoop()
        results: set[str] = set()

        loop.call_later(0, results.add, "zero_delay")
        loop.call_soon(results.add, "soon")
        loop.call_soon(loop.stop)
        loop.run_forever()

        # call_later(0) should behave like call_soon
        self.assertEqual(results, {"soon", "zero_delay"})
        loop.close()


class TestCallAt(TestCase):
    def test_call_at_returns_timer_handle(self) -> None:
        """Test that call_at returns a TimerHandle."""
        loop = compio.CompioLoop()
        when = loop.time() + 0.1
        handle = loop.call_at(when, lambda: None)
        self.assertIsInstance(handle, compio.TimerHandle)
        loop.close()

    def test_call_at_execution(self) -> None:
        """Test that call_at executes callback at specified time."""
        loop = compio.CompioLoop()
        results: list[str] = []

        when = loop.time() + 0.01
        loop.call_at(when, results.append, "scheduled")
        loop.call_at(when + 0.01, loop.stop)
        loop.run_forever()

        self.assertEqual(results, ["scheduled"])
        loop.close()

    def test_call_at_order(self) -> None:
        """Test that call_at callbacks execute in time order."""
        loop = compio.CompioLoop()
        results: list[int] = []
        base = loop.time()

        loop.call_at(base + 0.03, results.append, 3)
        loop.call_at(base + 0.01, results.append, 1)
        loop.call_at(base + 0.02, results.append, 2)
        loop.call_at(base + 0.04, loop.stop)
        loop.run_forever()

        self.assertEqual(results, [1, 2, 3])
        loop.close()

    def test_call_at_past_time(self) -> None:
        """Test call_at with time in the past executes immediately."""
        loop = compio.CompioLoop()
        results: list[str] = []

        # Schedule at a time in the past
        past_time = loop.time() - 1.0
        loop.call_at(past_time, results.append, "past")
        loop.call_soon(loop.stop)
        loop.run_forever()

        self.assertEqual(results, ["past"])
        loop.close()


class TestTimerHandle(TestCase):
    def test_timer_handle_when(self) -> None:
        """Test that TimerHandle.when() returns scheduled time."""
        loop = compio.CompioLoop()
        when = loop.time() + 0.5
        handle = loop.call_at(when, lambda: None)

        self.assertAlmostEqual(handle.when(), when, places=3)
        loop.close()

    def test_timer_handle_cancel(self) -> None:
        """Test that cancelled TimerHandle's callback is not executed."""
        loop = compio.CompioLoop()
        results: list[str] = []

        handle = loop.call_later(0.01, results.append, "cancelled")
        loop.call_later(0.02, results.append, "executed")
        handle.cancel()
        loop.call_later(0.03, loop.stop)
        loop.run_forever()

        self.assertEqual(results, ["executed"])
        loop.close()

    def test_timer_handle_cancelled_state(self) -> None:
        """Test TimerHandle.cancelled() returns correct state."""
        loop = compio.CompioLoop()
        handle = loop.call_later(0.1, lambda: None)

        self.assertFalse(handle.cancelled())
        handle.cancel()
        self.assertTrue(handle.cancelled())
        loop.close()

    def test_timer_handle_is_handle_subclass(self) -> None:
        """Test that TimerHandle is a subclass of Handle."""
        loop = compio.CompioLoop()
        handle = loop.call_later(0.1, lambda: None)

        self.assertIsInstance(handle, compio.Handle)
        self.assertIsInstance(handle, compio.TimerHandle)
        loop.close()

    def test_timer_handle_hashable(self) -> None:
        """Test that TimerHandle is hashable."""
        loop = compio.CompioLoop()
        handle1 = loop.call_later(0.1, lambda: None)
        handle2 = loop.call_later(0.2, lambda: None)

        # Should be hashable
        h1 = hash(handle1)
        h2 = hash(handle2)
        self.assertIsInstance(h1, int)
        self.assertIsInstance(h2, int)

        # Can be used in sets
        handle_set = {handle1, handle2}
        self.assertEqual(len(handle_set), 2)
        loop.close()


class TestTimerHandleCancellation(TestCase):
    def test_mass_cancel_cleanup(self) -> None:
        """Test that mass cancellation of timers triggers cleanup."""
        loop = compio.CompioLoop()
        results: list[str] = []

        # Create many timer handles and cancel most of them
        # This tests the MIN_SCHEDULED_TIMER_HANDLES and
        # MIN_CANCELLED_TIMER_HANDLES_FRACTION logic
        handles = []
        for i in range(150):
            h = loop.call_later(1.0, results.append, f"timer_{i}")
            handles.append(h)

        # Cancel more than 50% of the handles
        for i in range(100):
            handles[i].cancel()

        # Add a callback that will actually run
        loop.call_later(0.01, results.append, "survivor")
        loop.call_later(0.02, loop.stop)
        loop.run_forever()

        self.assertEqual(results, ["survivor"])
        loop.close()

    def test_cancel_multiple_times(self) -> None:
        """Test that cancelling a timer handle multiple times is safe."""
        loop = compio.CompioLoop()
        results: list[str] = []

        handle = loop.call_later(0.01, results.append, "should_not_run")

        # Cancel multiple times - should not cause issues
        handle.cancel()
        handle.cancel()
        handle.cancel()

        self.assertTrue(handle.cancelled())

        loop.call_later(0.02, results.append, "should_run")
        loop.call_later(0.03, loop.stop)
        loop.run_forever()

        self.assertEqual(results, ["should_run"])
        loop.close()

    def test_cancel_at_head_of_queue(self) -> None:
        """Test that cancelled timers at head of queue are properly removed."""
        loop = compio.CompioLoop()
        results: list[str] = []

        # Schedule timers in order
        h1 = loop.call_later(0.01, results.append, "first")
        loop.call_later(0.02, results.append, "second")
        loop.call_later(0.03, loop.stop)

        # Cancel the first one
        h1.cancel()

        loop.run_forever()

        self.assertEqual(results, ["second"])
        loop.close()


class TestCallLaterContextVars(TestCase):
    def test_call_later_copies_context(self) -> None:
        """Test that call_later copies current context."""
        loop = compio.CompioLoop()
        var: contextvars.ContextVar[str] = contextvars.ContextVar("test_var")
        results: list[str] = []

        var.set("original")

        def callback() -> None:
            results.append(var.get())

        loop.call_later(0.01, callback)

        # Change value after scheduling
        var.set("changed")

        loop.call_later(0.02, loop.stop)
        loop.run_forever()

        # Callback should see "original" since context was copied at schedule time
        self.assertEqual(results, ["original"])
        loop.close()

    def test_call_at_with_explicit_context(self) -> None:
        """Test call_at with explicit context argument."""
        loop = compio.CompioLoop()
        var: contextvars.ContextVar[str] = contextvars.ContextVar("test_var")
        results: list[str] = []

        def callback() -> None:
            results.append(var.get())

        # Create a context with a specific value
        var.set("in_context")
        ctx = contextvars.copy_context()

        # Schedule with explicit context
        when = loop.time() + 0.01
        loop.call_at(when, callback, context=ctx)
        loop.call_at(when + 0.01, loop.stop)
        loop.run_forever()

        self.assertEqual(results, ["in_context"])
        loop.close()


if sys.version_info >= (3, 12):
    class TestHandleGetContext(TestCase):
        def test_get_context_returns_context(self) -> None:
            """Test that Handle.get_context() returns the context."""
            loop = compio.CompioLoop()
            var: contextvars.ContextVar[str] = contextvars.ContextVar("test_var")

            var.set("test_value")
            handle = loop.call_soon(lambda: None)

            ctx = handle.get_context()
            self.assertIsInstance(ctx, contextvars.Context)
            # The context should contain the value set at schedule time
            self.assertEqual(ctx.run(var.get), "test_value")
            loop.close()

        def test_timer_handle_get_context(self) -> None:
            """Test that TimerHandle.get_context() returns the context."""
            loop = compio.CompioLoop()
            var: contextvars.ContextVar[str] = contextvars.ContextVar("test_var")

            var.set("timer_value")
            handle = loop.call_later(0.1, lambda: None)

            ctx = handle.get_context()
            self.assertIsInstance(ctx, contextvars.Context)
            self.assertEqual(ctx.run(var.get), "timer_value")
            loop.close()

        def test_get_context_with_explicit_context(self) -> None:
            """Test get_context with explicitly provided context."""
            loop = compio.CompioLoop()
            var: contextvars.ContextVar[str] = contextvars.ContextVar("test_var")

            var.set("explicit_value")
            explicit_ctx = contextvars.copy_context()

            var.set("changed_value")
            handle = loop.call_soon(lambda: None, context=explicit_ctx)

            ctx = handle.get_context()
            self.assertEqual(ctx.run(var.get), "explicit_value")
            loop.close()


class TestExceptionHandler(TestCase):
    def test_get_exception_handler_default(self) -> None:
        """Test that get_exception_handler returns None by default."""
        loop = compio.CompioLoop()
        self.assertIsNone(loop.get_exception_handler())
        loop.close()

    def test_set_exception_handler(self) -> None:
        """Test setting a custom exception handler."""
        loop = compio.CompioLoop()

        def custom_handler(loop: asyncio.AbstractEventLoop, context: dict) -> None:
            pass

        loop.set_exception_handler(custom_handler)
        self.assertIs(loop.get_exception_handler(), custom_handler)
        loop.close()

    def test_set_exception_handler_none(self) -> None:
        """Test resetting exception handler to None."""
        loop = compio.CompioLoop()

        def custom_handler(loop: asyncio.AbstractEventLoop, context: dict) -> None:
            pass

        loop.set_exception_handler(custom_handler)
        loop.set_exception_handler(None)
        self.assertIsNone(loop.get_exception_handler())
        loop.close()

    def test_set_exception_handler_not_callable(self) -> None:
        """Test that setting non-callable raises TypeError."""
        loop = compio.CompioLoop()

        with self.assertRaises(TypeError):
            loop.set_exception_handler("not a callable")  # type: ignore

        loop.close()

    def test_callback_exception_calls_exception_handler(self) -> None:
        """Test that exceptions in callbacks invoke the exception handler."""
        loop = compio.CompioLoop()
        exceptions_handled: list[dict] = []

        def custom_handler(loop: asyncio.AbstractEventLoop, context: dict) -> None:
            exceptions_handled.append(context)

        def bad_callback() -> None:
            raise ValueError("test error")

        loop.set_exception_handler(custom_handler)
        loop.call_soon(bad_callback)
        loop.call_soon(loop.stop)
        loop.run_forever()

        self.assertEqual(len(exceptions_handled), 1)
        self.assertEqual(exceptions_handled[0]["message"], "Exception in callback")
        self.assertIsInstance(exceptions_handled[0]["exception"], ValueError)
        loop.close()

    def test_callback_exception_with_default_handler(self) -> None:
        """Test that exceptions with default handler are logged."""
        loop = compio.CompioLoop()
        results: list[str] = []

        def bad_callback() -> None:
            raise ValueError("test error")

        # Default handler logs the exception, callback execution continues
        loop.call_soon(bad_callback)
        loop.call_soon(results.append, "after_error")
        loop.call_soon(loop.stop)
        loop.run_forever()

        self.assertEqual(results, ["after_error"])
        loop.close()

    def test_system_exit_propagates(self) -> None:
        """Test that SystemExit propagates through exception handling."""
        loop = compio.CompioLoop()

        def raise_system_exit() -> None:
            raise SystemExit(42)

        loop.call_soon(raise_system_exit)

        with self.assertRaises(SystemExit) as cm:
            loop.run_forever()

        self.assertEqual(cm.exception.code, 42)
        loop.close()

    def test_keyboard_interrupt_propagates(self) -> None:
        """Test that KeyboardInterrupt propagates through exception handling."""
        loop = compio.CompioLoop()

        def raise_keyboard_interrupt() -> None:
            raise KeyboardInterrupt()

        loop.call_soon(raise_keyboard_interrupt)

        with self.assertRaises(KeyboardInterrupt):
            loop.run_forever()

        loop.close()

    def test_exception_handler_receives_handle_context(self) -> None:
        """Test that exception handler can access handle's context."""
        loop = compio.CompioLoop()
        var: contextvars.ContextVar[str] = contextvars.ContextVar("test_var")
        captured_values: list[str] = []

        def custom_handler(loop: asyncio.AbstractEventLoop, context: dict) -> None:
            # The handler runs in the handle's context
            captured_values.append(var.get())

        def bad_callback() -> None:
            raise ValueError("test error")

        var.set("callback_context_value")
        ctx = contextvars.copy_context()

        loop.set_exception_handler(custom_handler)
        var.set("different_value")
        loop.call_soon(bad_callback, context=ctx)
        loop.call_soon(loop.stop)
        loop.run_forever()

        # The exception handler should run in the handle's context
        self.assertEqual(captured_values, ["callback_context_value"])
        loop.close()

    def test_call_exception_handler_directly(self) -> None:
        """Test calling call_exception_handler directly."""
        loop = compio.CompioLoop()
        exceptions_handled: list[dict] = []

        def custom_handler(loop: asyncio.AbstractEventLoop, context: dict) -> None:
            exceptions_handled.append(context)

        loop.set_exception_handler(custom_handler)
        loop.call_exception_handler({"message": "Test message", "custom_key": "value"})

        self.assertEqual(len(exceptions_handled), 1)
        self.assertEqual(exceptions_handled[0]["message"], "Test message")
        self.assertEqual(exceptions_handled[0]["custom_key"], "value")
        loop.close()

    def test_exception_in_custom_handler_falls_back(self) -> None:
        """Test that exception in custom handler falls back to default."""
        loop = compio.CompioLoop()
        results: list[str] = []

        def bad_handler(loop: asyncio.AbstractEventLoop, context: dict) -> None:
            raise RuntimeError("handler error")

        def bad_callback() -> None:
            raise ValueError("callback error")

        loop.set_exception_handler(bad_handler)
        loop.call_soon(bad_callback)
        loop.call_soon(results.append, "continued")
        loop.call_soon(loop.stop)
        loop.run_forever()

        # Loop should continue even if custom handler fails
        self.assertEqual(results, ["continued"])
        loop.close()


class TestCallLaterExceptions(TestCase):
    def test_exception_in_timer_callback(self) -> None:
        """Test that exceptions in timer callbacks invoke exception handler."""
        loop = compio.CompioLoop()
        exceptions_handled: list[dict] = []

        def custom_handler(loop: asyncio.AbstractEventLoop, context: dict) -> None:
            exceptions_handled.append(context)

        def bad_callback() -> None:
            raise ValueError("timer error")

        loop.set_exception_handler(custom_handler)
        loop.call_later(0.01, bad_callback)
        loop.call_later(0.02, loop.stop)
        loop.run_forever()

        self.assertEqual(len(exceptions_handled), 1)
        self.assertIsInstance(exceptions_handled[0]["exception"], ValueError)
        loop.close()

    def test_timer_exception_does_not_stop_loop(self) -> None:
        """Test that timer exceptions don't stop the loop."""
        loop = compio.CompioLoop()
        results: list[str] = []

        def bad_callback() -> None:
            raise ValueError("timer error")

        def custom_handler(loop: asyncio.AbstractEventLoop, context: dict) -> None:
            results.append("exception_handled")

        loop.set_exception_handler(custom_handler)
        loop.call_later(0.01, bad_callback)
        loop.call_later(0.02, results.append, "after")
        loop.call_later(0.03, loop.stop)
        loop.run_forever()

        self.assertEqual(results, ["exception_handled", "after"])
        loop.close()
