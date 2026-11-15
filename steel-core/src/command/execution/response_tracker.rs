//! Vanilla `CommandResponseTracker`.

use text_components::TextComponent;

use super::CommandSource;
use crate::command::brigadier::CommandSyntaxError;

/// Vanilla `CommandResponseTracker.ElementType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ElementType {
    /// Every tracked element counts.
    Any,
    /// Only elements tracked with a non-zero value count.
    NonZero,
}

/// Vanilla `CommandResponseTracker`: counts the elements a command touched
/// and remembers the only one, for single and multiple feedback.
#[derive(Debug)]
pub(crate) struct ResponseTracker<T> {
    total_value: i32,
    only_element: Option<T>,
    element_count: i32,
    only_non_zero_element: Option<T>,
    non_zero_element_count: i32,
}

impl<T> Default for ResponseTracker<T> {
    fn default() -> Self {
        Self {
            total_value: 0,
            only_element: None,
            element_count: 0,
            only_non_zero_element: None,
            non_zero_element_count: 0,
        }
    }
}

impl<T: Clone> ResponseTracker<T> {
    /// Vanilla `track(element, value)`.
    pub(crate) fn track(&mut self, element: &T, value: i32) {
        self.total_value = self.total_value.wrapping_add(value);
        self.element_count += 1;
        self.only_element = (self.element_count == 1).then(|| element.clone());
        if value != 0 {
            self.non_zero_element_count += 1;
            self.only_non_zero_element =
                (self.non_zero_element_count == 1).then(|| element.clone());
        }
    }

    /// Vanilla `track(element)`, a value of one.
    pub(crate) fn track_one(&mut self, element: &T) {
        self.track(element, 1);
    }

    const fn only(&self, element_type: ElementType) -> Option<&T> {
        match element_type {
            ElementType::Any => self.only_element.as_ref(),
            ElementType::NonZero => self.only_non_zero_element.as_ref(),
        }
    }

    const fn count(&self, element_type: ElementType) -> i32 {
        match element_type {
            ElementType::Any => self.element_count,
            ElementType::NonZero => self.non_zero_element_count,
        }
    }

    /// Vanilla `sendFeedback`: fails with `on_zero` when nothing counted,
    /// otherwise sends the single or multiple message and returns the total.
    ///
    /// The single message gets the element and the total, the multiple one
    /// the element count and the total.
    pub(crate) fn send_feedback(
        &self,
        source: &CommandSource,
        broadcast: bool,
        element_type: ElementType,
        on_zero: Option<fn() -> CommandSyntaxError>,
        single: impl FnOnce(&T, i32) -> TextComponent,
        multiple: impl FnOnce(i32, i32) -> TextComponent,
    ) -> Result<i32, CommandSyntaxError> {
        let count = self.count(element_type);
        if let Some(on_zero) = on_zero
            && count == 0
        {
            return Err(on_zero());
        }
        let message = match self.only(element_type) {
            Some(element) => single(element, self.total_value),
            None => multiple(count, self.total_value),
        };
        source.send_success(&message, broadcast);
        Ok(self.total_value)
    }
}

#[cfg(test)]
mod tests {
    use super::{ElementType, ResponseTracker};

    #[test]
    fn tracker_remembers_only_the_single_element_of_each_kind() {
        let mut tracker = ResponseTracker::default();
        tracker.track(&"a", 0);
        tracker.track(&"b", 3);

        assert_eq!(tracker.only(ElementType::Any), None);
        assert_eq!(tracker.count(ElementType::Any), 2);
        assert_eq!(tracker.only(ElementType::NonZero), Some(&"b"));
        assert_eq!(tracker.count(ElementType::NonZero), 1);
        assert_eq!(tracker.total_value, 3);
    }
}
