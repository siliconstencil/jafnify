//! The four code boxes of the GUI as plain state: which digit goes where and
//! where the focus moves next. egui only draws the boxes and forwards events.

#[derive(Debug, Clone, Default)]
pub struct CodeBoxes {
    boxes: [String; 4],
}

impl CodeBoxes {
    pub fn text(&self, i: usize) -> &str {
        &self.boxes[i]
    }

    /// The field of box `i` now reads `text`. Keeps the last digit typed (so
    /// typing over a filled box replaces it) and drops everything else.
    /// Returns the box to focus next, if a digit was typed.
    pub fn input(&mut self, i: usize, text: &str) -> Option<usize> {
        let digits: Vec<char> = text.chars().filter(char::is_ascii_digit).collect();
        let Some(&digit) = digits.last() else {
            self.boxes[i].clear();
            return None;
        };
        let new = digit.to_string();
        let advanced = self.boxes[i] != new || digits.len() > 1;
        self.boxes[i] = new;
        (advanced && i < 3).then_some(i + 1)
    }

    /// Backspace pressed in box `i`. In an empty box it clears the previous
    /// one and returns it as the box to focus; a filled box is emptied by the
    /// text field itself.
    pub fn backspace(&mut self, i: usize) -> Option<usize> {
        if i == 0 || !self.boxes[i].is_empty() {
            return None;
        }
        self.boxes[i - 1].clear();
        Some(i - 1)
    }

    /// Replaces all boxes with the first four digits of `text`. Returns the
    /// box to focus; None (and no change) when `text` has no digit.
    pub fn paste(&mut self, text: &str) -> Option<usize> {
        let digits: Vec<char> = text.chars().filter(char::is_ascii_digit).take(4).collect();
        if digits.is_empty() {
            return None;
        }
        self.clear();
        for (i, d) in digits.iter().enumerate() {
            self.boxes[i] = d.to_string();
        }
        Some(digits.len().min(3))
    }

    /// The digits entered so far, in box order.
    pub fn code(&self) -> String {
        self.boxes.concat()
    }

    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.code().len()
    }

    pub fn is_complete(&self) -> bool {
        self.len() == 4
    }

    pub fn clear(&mut self) {
        for b in &mut self.boxes {
            b.clear();
        }
    }
}
