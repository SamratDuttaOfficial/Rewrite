"""Background workers for the UI."""
import threading
import traceback


class Lane(threading.Thread):
    """Background worker that only runs the most recent job submitted to it."""

    def __init__(self, app, name):
        super().__init__(daemon=True, name=name)
        self.app, self.cv, self.job = app, threading.Condition(), None
        self.active = False  # a job is running or waiting
        self.start()

    def submit(self, fn, done=None):
        with self.cv:
            self.job = (fn, done)
            self.active = True
            self.cv.notify()

    def run(self):
        while True:
            with self.cv:
                while self.job is None:
                    self.cv.wait()
                fn, done = self.job
                self.job = None
            try:
                res, err = fn(), None
            except Exception as e:  # report to the UI instead of dying
                traceback.print_exc()
                res, err = None, e
            with self.cv:
                self.active = self.job is not None
            if done:
                self.app.post(lambda d=done, r=res, e=err: d(r, e))
