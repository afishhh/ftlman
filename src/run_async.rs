trait UiRunAsync {
    fn run_async<F: Future>(ui: Ui, task: F) -> RunAsync<F>;
}

impl UiRunAsync for Ui {
    fn run_async<F: Future>(ui: Ui, task: F) -> RunAsync<F> {
        RunAsync::Waiting(Box::pin(task))
    }
}

enum RunAsync<F: Future> {
    Waiting(Pin<Box<F>>),
    Done(F::Output),
    Empty,
}

enum RunAsyncState<O> {
    Waiting,
    Done(O),
    Taken,
}

impl<F: Future> RunAsync<F> {
    fn poll(&mut self) {
        if let RunAsync::Waiting(future) = self {
            if let std::task::Poll::Ready(result) = future
                .as_mut()
                .poll(&mut Context::from_waker(futures::task::noop_waker_ref()))
            {
                *self = RunAsync::Done(result)
            }
        }
    }

    pub fn get(&mut self) -> Option<&F::Output> {
        self.poll();

        if let RunAsync::Done(ref result) = *self {
            Some(result)
        } else {
            None
        }
    }

    pub fn take(&mut self) -> Option<F::Output> {
        self.poll();

        if let RunAsync::Done(_) = *self {
            let r = std::mem::replace(self, RunAsync::Empty);
            if let RunAsync::Done(val) = r {
                Some(val)
            } else {
                unreachable!()
            }
        } else {
            None
        }
    }

    pub fn get_state(&mut self) -> RunAsyncState<&F::Output> {
        match self {
            RunAsync::Waiting(_) => RunAsyncState::Waiting,
            RunAsync::Done(ref value) => RunAsyncState::Done(value),
            RunAsync::Empty => RunAsyncState::Taken,
        }
    }
}
