from pathlib import Path
from reportlab.lib.pagesizes import A4
from reportlab.lib.units import mm
from reportlab.lib import colors
from reportlab.pdfgen import canvas
from reportlab.lib.styles import ParagraphStyle
from reportlab.platypus import Paragraph

out = Path(__file__).resolve().parent
pdf_path = out / 'shuestand-flyer-de-a4-8up.pdf'
qr_path = out / 'shuestand-qr.png'

C_BG = colors.HexColor('#f8fafc')
C_CARD = colors.white
C_BORDER = colors.HexColor('#dbe3ea')
C_TITLE = colors.HexColor('#141327')
C_TEXT = colors.HexColor('#1f2937')
C_SUBTLE = colors.HexColor('#475569')
C_ACCENT_WARM = colors.HexColor('#d6c3a8')
C_ACCENT_COOL = colors.HexColor('#98aed8')

w, h = A4
c = canvas.Canvas(str(pdf_path), pagesize=A4)
c.setFillColor(C_BG)
c.rect(0, 0, w, h, fill=1, stroke=0)

page_margin = 10 * mm
gutter_x = 4 * mm
gutter_y = 4 * mm
cols, rows = 2, 4

inner_w = w - 2 * page_margin
inner_h = h - 2 * page_margin
card_w = (inner_w - gutter_x * (cols - 1)) / cols
card_h = (inner_h - gutter_y * (rows - 1)) / rows

c.setStrokeColor(colors.HexColor('#cbd5e1'))
c.setLineWidth(0.4)
for i in range(1, cols):
    x = page_margin + i * card_w + (i - 0.5) * gutter_x
    c.line(x, page_margin - 2*mm, x, page_margin + 2*mm)
    c.line(x, h - page_margin - 2*mm, x, h - page_margin + 2*mm)
for j in range(1, rows):
    y = page_margin + j * card_h + (j - 0.5) * gutter_y
    c.line(page_margin - 2*mm, y, page_margin + 2*mm, y)
    c.line(w - page_margin - 2*mm, y, w - page_margin + 2*mm, y)


def paragraph(text, x, y_top, width, size=7.2, leading=None, bold=False, color=C_TEXT):
    style = ParagraphStyle(
        name='p',
        fontName='Helvetica-Bold' if bold else 'Helvetica',
        fontSize=size,
        leading=leading or size * 1.2,
        alignment=1,
        textColor=color,
        wordWrap='CJK',
    )
    p = Paragraph(text, style)
    pw, ph = p.wrap(width, 200*mm)
    p.drawOn(c, x + (width - pw)/2, y_top - ph)
    return y_top - ph

for r in range(rows):
    for col in range(cols):
        x = page_margin + col * (card_w + gutter_x)
        y = h - page_margin - (r + 1) * card_h - r * gutter_y

        c.setFillColor(C_CARD)
        c.setStrokeColor(C_BORDER)
        c.setLineWidth(1)
        c.roundRect(x, y, card_w, card_h, 8, fill=1, stroke=1)

        stripe_h = 2.4 * mm
        c.setFillColor(C_ACCENT_WARM)
        c.roundRect(x + 2.5*mm, y + card_h - 4.2*mm, (card_w-5*mm)/2, stripe_h, 1.5, fill=1, stroke=0)
        c.setFillColor(C_ACCENT_COOL)
        c.roundRect(x + 2.5*mm + (card_w-5*mm)/2, y + card_h - 4.2*mm, (card_w-5*mm)/2, stripe_h, 1.5, fill=1, stroke=0)

        pad = 3.5 * mm
        content_w = card_w - 2 * pad
        y_cursor = y + card_h - 7.4 * mm

        c.setFillColor(C_TITLE)
        c.setFont('Helvetica-Bold', 11)
        c.drawCentredString(x + card_w/2, y_cursor, 'Shuestand')

        y_cursor -= 4.8 * mm
        y_cursor = paragraph('Bitcoin ↔ Cashu einfach tauschen', x + pad, y_cursor, content_w, size=6.9, color=C_TEXT)

        y_cursor -= 2.0 * mm
        qr = 23 * mm
        qr_x = x + (card_w - qr) / 2
        qr_y = y_cursor - qr
        c.setFillColor(colors.HexColor('#eef2f7'))
        c.setStrokeColor(C_ACCENT_COOL)
        c.roundRect(qr_x - 1.8*mm, qr_y - 1.8*mm, qr + 3.6*mm, qr + 3.6*mm, 4, fill=1, stroke=1)
        c.drawImage(str(qr_path), qr_x, qr_y, qr, qr, preserveAspectRatio=True, mask='auto')

        y_cursor = qr_y - 2.0 * mm
        y_cursor = paragraph('shuestand.mountainlake.io', x + pad, y_cursor, content_w, size=6.5, color=colors.HexColor('#334155'), bold=True)
        y_cursor -= 1.2 * mm
        paragraph('Problems? Contact marius@mountainlake.io', x + pad, y_cursor, content_w, size=5.7, color=C_SUBTLE)

c.showPage()
c.save()
print(pdf_path)
