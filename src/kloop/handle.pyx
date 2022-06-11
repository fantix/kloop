# Copyright (c) 2022 Fantix King  https://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


cdef class Handle:
    def __init__(self, callback, args, loop, context=None):
        if context is None:
            context = contextvars.copy_context()
        self.context = context
        self.loop = loop
        self.callback = callback
        self.args = args
        self.repr = None
        self.cb.handle = <PyObject*>self
        # if self._loop.get_debug():
        #     self._source_traceback = format_helpers.extract_stack(
        #         sys._getframe(1))
        # else:
        #     self._source_traceback = None

    def _repr_info(self):
        info = [self.__class__.__name__]
        if self.cb.mask & CANCELLED_MASK:
            info.append('cancelled')
        if self.callback is not None:
            info.append(_format_callback_source(self.callback, self.args))
    #     if self._source_traceback:
    #         frame = self._source_traceback[-1]
    #         info.append(f'created at {frame[0]}:{frame[1]}')
        return info

    def __repr__(self):
    #     if self._repr is not None:
    #         return self._repr
        info = self._repr_info()
        return '<{}>'.format(' '.join(info))

    def cancel(self):
        if self.cb.mask & CANCELLED_MASK == 0:
            self.cb.mask |= CANCELLED_MASK
            # if self._loop.get_debug():
            #     # Keep a representation in debug mode to keep callback and
            #     # parameters. For example, to log the warning
            #     # "Executing <Handle...> took 2.5 second"
            #     self._repr = repr(self)
            self.callback = None
            self.args = None

    def cancelled(self):
        return self.cb.mask & CANCELLED_MASK == 1

    cdef run(self):
        try:
            self.context.run(self.callback, *self.args)
        except (SystemExit, KeyboardInterrupt):
            raise
        except BaseException as exc:
            cb = _format_callback_source(self.callback, self.args)
            msg = f'Exception in callback {cb}'
            context = {
                'message': msg,
                'exception': exc,
                'handle': self,
            }
            if self.source_traceback:
                context['source_traceback'] = self.source_traceback
            self.loop.call_exception_handler(context)
        self = None  # Needed to break cycles when an exception occurs.


cdef class TimerHandle(Handle):
    """Object returned by timed callback registration methods."""

    def __init__(self, when, callback, args, loop, context=None):
        assert when is not None
        super().__init__(callback, args, loop, context)
        if self.source_traceback:
            del self.source_traceback[-1]
        self.cb.when = when

    # def _repr_info(self):
    #     info = super()._repr_info()
    #     pos = 2 if self._cancelled else 1
    #     info.insert(pos, f'when={self._when}')
    #     return info

    def cancel(self):
        if self.cb.mask & (CANCELLED_MASK | SCHEDULED_MASK) == SCHEDULED_MASK:
            self.loop.loop.timer_cancelled_count += 1
        super().cancel()

    def when(self):
        return self.cb.when


def _get_function_source(func):
    func = inspect.unwrap(func)
    if inspect.isfunction(func):
        code = func.__code__
        return (code.co_filename, code.co_firstlineno)
    if isinstance(func, functools.partial):
        return _get_function_source(func.func)
    if isinstance(func, functools.partialmethod):
        return _get_function_source(func.func)
    return None


def _format_callback_source(func, args):
    func_repr = _format_callback(func, args, None)
    source = _get_function_source(func)
    if source:
        func_repr += f' at {source[0]}:{source[1]}'
    return func_repr


def _format_args_and_kwargs(args, kwargs):
    """Format function arguments and keyword arguments.

    Special case for a single parameter: ('hello',) is formatted as ('hello').
    """
    # use reprlib to limit the length of the output
    items = []
    if args:
        items.extend(reprlib.repr(arg) for arg in args)
    if kwargs:
        items.extend(f'{k}={reprlib.repr(v)}' for k, v in kwargs.items())
    return '({})'.format(', '.join(items))


def _format_callback(func, args, kwargs, suffix=''):
    if isinstance(func, functools.partial):
        suffix = _format_args_and_kwargs(args, kwargs) + suffix
        return _format_callback(func.func, func.args, func.keywords, suffix)

    if hasattr(func, '__qualname__') and func.__qualname__:
        func_repr = func.__qualname__
    elif hasattr(func, '__name__') and func.__name__:
        func_repr = func.__name__
    else:
        func_repr = repr(func)

    func_repr += _format_args_and_kwargs(args, kwargs)
    if suffix:
        func_repr += suffix
    return func_repr
