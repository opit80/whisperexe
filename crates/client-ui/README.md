# client-ui — durum overlay'i durum makinesi

`client` çekirdekteki olayları tek odaklı overlay'e çevirir:
kayıt göstergesi, bakiye rozeti, kuyruk sırası, hata/toast,
güncelleme halleri. Tauri/WebView2 penceresi bu makineyi dinler;
görsel kabuk sonradan eklenir.

Tasarım kararları (skill'lere göre): `src/lib.rs` başındaki
Intent/Hierarchy/Palette/Depth/Surfaces/Typography/Spacing notuna bakın.
