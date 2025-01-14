/*
    This file is part of Eruption.

    Eruption is free software: you can redistribute it and/or modify
    it under the terms of the GNU General Public License as published by
    the Free Software Foundation, either version 3 of the License, or
    (at your option) any later version.

    Eruption is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
    GNU General Public License for more details.

    You should have received a copy of the GNU General Public License
    along with Eruption.  If not, see <http://www.gnu.org/licenses/>.

    Copyright (c) 2019-2022, The Eruption Development Team
*/

pub struct SimpleMappingAssistant {
    pub page: usize,
}

impl SimpleMappingAssistant {
    pub fn new() -> Self {
        Self { page: 0 }
    }
}

impl super::Assistant for SimpleMappingAssistant {
    fn title(&self) -> &'static str {
        "Create a simple key mapping"
    }

    fn description(&self) -> &'static str {
        "Creates a simple key mapping"
    }

    fn previous_step(&mut self) -> super::Result<()> {
        self.page -= 1;
        Ok(())
    }

    fn next_step(&mut self) -> super::Result<()> {
        self.page += 1;
        Ok(())
    }

    fn render_page(&mut self) -> super::Result<()> {
        Ok(())
    }
}
