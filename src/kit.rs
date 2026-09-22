//! Vocabulário visual compartilhado pelos addons: o que a tabela de processos e os cards
//! já fazem, embalado pra Partida, Desperdício, Térmico, Telas e Limpeza não parecerem
//! egui cru dentro do card.

use egui::{Color32, RichText, Stroke, Vec2};

use crate::app::{ACCENT, ACCENT_BG, BG, MUTED, SURFACE_HI, TEXT};

pub const ROW_R: f32 = 8.0;

/// Linha de introdução da visão (o título já está no cabeçalho da janela).
pub fn intro(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(MUTED).size(12.5)).wrap());
}

/// Fileira de controles com botões em pílula.
pub fn toolbar<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.spacing_mut().button_padding = Vec2::new(10.0, 5.0);
        add(ui)
    })
    .inner
}

pub fn button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).size(12.5))
        .fill(SURFACE_HI)
        .stroke(Stroke::NONE)
        .corner_radius(ROW_R)
}

pub fn primary(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).size(12.5).color(TEXT))
        .fill(ACCENT_BG)
        .stroke(Stroke::new(1.0_f32, ACCENT.gamma_multiply(0.7)))
        .corner_radius(ROW_R)
}

pub fn danger(text: &str) -> egui::Button<'static> {
    egui::Button::new(
        RichText::new(text.to_owned())
            .size(12.5)
            .color(Color32::WHITE),
    )
    .fill(Color32::from_rgb(170, 50, 50))
    .stroke(Stroke::NONE)
    .corner_radius(ROW_R)
}

/// Uma linha da lista em caixa (estilo boxed-list): fundo da janela dentro do card.
pub fn row<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(BG)
        .corner_radius(ROW_R)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().button_padding = Vec2::new(10.0, 4.0);
            add(ui)
        })
        .inner
}

/// Chip de estado: pílula com a cor a 18%.
pub fn badge(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .corner_radius(999.0)
        .inner_margin(egui::Margin::symmetric(8, 1))
        .show(ui, |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            ui.label(RichText::new(text).size(11.0).color(color));
        })
        .response
}

/// Título + subtítulo em duas linhas, como as linhas da tabela de processos.
pub fn two_lines(ui: &mut egui::Ui, title: impl Into<RichText>, sub: &str) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.add(egui::Label::new(title.into().size(13.0)).truncate());
        if !sub.is_empty() {
            ui.add(egui::Label::new(RichText::new(sub).size(11.5).color(MUTED)).truncate());
        }
    });
}

/// Texto secundário, alinhado com o resto.
pub fn muted(text: &str) -> RichText {
    RichText::new(text).size(11.5).color(MUTED)
}
