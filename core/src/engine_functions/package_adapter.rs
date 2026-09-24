//! Executable adapter contract. Policy behavior is code, never a semantic DSL.
use crate::{multiplexer::HookResult, v8host::S2FunctionValue};

#[derive(Clone, Copy)]
pub(crate) struct SubscriberDelivery {
    pub action: HookResult,
    pub return_value: Option<S2FunctionValue>,
    pub frame_revision: u64,
}
#[derive(Clone, Copy)]
pub(crate) enum SuppressAction {
    Handled,
    Stop,
}
pub(crate) enum PreDecision {
    Continue,
    Changed,
    Suppress {
        action: SuppressAction,
        return_value: Option<S2FunctionValue>,
    },
}
pub(crate) trait SubscriberCursor {
    fn invoke_next(&mut self) -> Result<Option<SubscriberDelivery>, String>;
}
pub(crate) struct AdapterDispatch<'a> {
    pub cursor: &'a mut dyn SubscriberCursor,
}
pub(crate) trait DispatchAdapter {
    fn pre(&self, dispatch: &mut AdapterDispatch<'_>) -> Result<PreDecision, String>;
    fn post(&self, dispatch: &mut AdapterDispatch<'_>) -> Result<(), String>;
}
/// Invalid decisions have already been rejected by the typed callback boundary.
/// Equal-strength decisions retain the first typed return in registration order.
pub(crate) struct GenericAdapter;
impl DispatchAdapter for GenericAdapter {
    fn pre(&self, dispatch: &mut AdapterDispatch<'_>) -> Result<PreDecision, String> {
        let mut best = PreDecision::Continue;
        let mut strength = HookResult::Continue;
        let mut revision = 0;
        while let Some(delivery) = dispatch.cursor.invoke_next()? {
            if delivery.frame_revision < revision {
                return Err("frame revision regressed".into());
            }
            revision = delivery.frame_revision;
            if delivery.action > strength {
                strength = delivery.action;
                best = match delivery.action {
                    HookResult::Continue => PreDecision::Continue,
                    HookResult::Changed => PreDecision::Changed,
                    HookResult::Handled | HookResult::Stop => PreDecision::Suppress {
                        action: if delivery.action == HookResult::Stop {
                            SuppressAction::Stop
                        } else {
                            SuppressAction::Handled
                        },
                        return_value: delivery.return_value,
                    },
                };
            }
            if delivery.action == HookResult::Stop {
                break;
            }
        }
        Ok(best)
    }
    fn post(&self, dispatch: &mut AdapterDispatch<'_>) -> Result<(), String> {
        while dispatch.cursor.invoke_next()?.is_some() {}
        Ok(())
    }
}
