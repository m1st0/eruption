/*  SPDX-License-Identifier: GPL-3.0-or-later  */

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

use egui::CentralPanel;

#[derive(Default)]
pub struct AboutPage {}

impl AboutPage {
    pub fn new() -> Self {
        Self {}
    }

    pub fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        CentralPanel::default().show(ctx, |ui| {
            ui.vertical(|ui| {
                ui.heading("About Pyroclasm UI for Eruption");

                ui.separator();

                ui.label("Authors:");
                ui.label("X3n0m0rph59 <x3n0m0rph59@gmail.com>");
                ui.label("The Eruption Development Team");

                ui.label("GPL3 or later");

                ui.spacing();
            });
        });
    }
}
