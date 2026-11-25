module app.active_request_context;

import std.task.{run, start, await, Scheduler, Async};

effect Provider {
    action value(id: i32) -> i32;
}

agent Service = {};

impl Service {
    public flow answer(self, id: i32) -> i32 ![Provider] {
        perform Async.yield();
        return perform Provider.value(id);
    }
}

flow main(args: Array<string>) -> i32 {
    return run((scheduler: Scheduler) => {
        let service = Service {};
        let first = start(() => {
            return handle { service.answer(1) } with {
                Provider.value(id) => { resume 100 + id; }
            };
        });
        let second = start(() => {
            return handle { service.answer(2) } with {
                Provider.value(id) => { resume 200 + id; }
            };
        });
        let a = await(first);
        let b = await(second);
        if a == 101 && b == 202 {
            return 0;
        }
        return 1;
    });
}

agent RequestGate = {
    entered: bool,
    stopped: bool,
};

impl RequestGate {
    public flow enter(var self) -> unit ![] {
        self.entered = true;
    }

    public flow stop(var self) -> unit ![] {
        self.stopped = true;
    }

    public flow is_entered(self) -> bool ![] {
        return self.entered;
    }

    public flow is_stopped(self) -> bool ![] {
        return self.stopped;
    }
}

flow cancel_case(args: Array<string>) -> i32 {
    return run((scheduler: Scheduler) => {
        let service = Service {};
        let gate = RequestGate { entered = false, stopped = false };
        let second = start(() => {
            return handle { service.answer(2) } with {
                Provider.value(id) => {
                    while !gate.is_stopped() {
                        perform Async.yield();
                    }
                    resume 200 + id;
                }
            };
        });
        let winner = std.task.race<i32, unit>([
            () => {
                return handle { service.answer(1) } with {
                    Provider.value(id) => {
                        gate.enter();
                        while !gate.is_stopped() {
                            perform Async.yield();
                        }
                        resume 100 + id;
                    }
                };
            },
            () => {
                while !gate.is_entered() {
                    perform Async.yield();
                }
                return 7;
            },
        ]);
        gate.stop();
        let reply = await(second);
        let next = handle { service.answer(3) } with {
            Provider.value(id) => { resume 300 + id; }
        };
        if winner == 7 && reply == 202 && next == 303 {
            return 0;
        }
        return 1;
    });
}

flow shared_handler_cancel_case(args: Array<string>) -> i32 {
    let service = Service {};
    let gate = RequestGate { entered = false, stopped = false };
    return handle {
        run((scheduler: Scheduler) => {
            let second = start(() => {
                while !gate.is_entered() {
                    perform Async.yield();
                }
                return service.answer(2);
            });
            let winner = std.task.race<i32, unit>([
                () => { return service.answer(1); },
                () => {
                    while !gate.is_entered() {
                        perform Async.yield();
                    }
                    return 7;
                },
            ]);
            gate.stop();
            let reply = await(second);
            let next = service.answer(3);
            if winner == 7 && reply == 202 && next == 303 {
                return 0;
            }
            return 1;
        })
    } with {
        Provider.value(id) => {
            if id == 1 {
                gate.enter();
                while !gate.is_stopped() {
                    perform Async.yield();
                }
            }
            resume id * 101;
        }
    };
}
