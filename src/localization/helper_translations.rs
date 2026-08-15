use super::LanguageId;

// The helper editors arrived as one large feature, so their catalogue lives
// together to keep every locale complete and make omissions easy to spot.
pub(super) fn text(language: LanguageId, english: &'static str) -> Option<&'static str> {
    let locale = match language {
        LanguageId::English => return None,
        LanguageId::Dutch => 0,
        LanguageId::Spanish => 1,
        LanguageId::French => 2,
        LanguageId::German => 3,
        LanguageId::Japanese => 4,
        LanguageId::Korean => 5,
        LanguageId::TraditionalChinese => 6,
        LanguageId::SimplifiedChinese => 7,
        LanguageId::Russian => 8,
        LanguageId::PortugueseBrazil => 9,
    };

    TRANSLATIONS
        .iter()
        .find(|(key, _)| *key == english)
        .map(|(_, translations)| translations[locale])
}

// Dutch, Spanish, French, German, Japanese, Korean, Traditional Chinese,
// Simplified Chinese, Russian, Brazilian Portuguese.
const TRANSLATIONS: &[(&str, [&str; 10])] = &[
    (
        "has been changed, what would you like to do?",
        ["is gewijzigd, wat wilt u doen?", "ha cambiado, ¿qué desea hacer?", "a été modifié, que souhaitez-vous faire ?", "wurde geändert. Was möchten Sie tun?", "が変更されました。どうしますか？", "이(가) 변경되었습니다. 어떻게 하시겠습니까?", "已變更，您想要怎麼做？", "已更改，您想怎么做？", "была изменена. Что вы хотите сделать?", "foi alterado. O que você deseja fazer?"],
    ),
    ("Automatic number", ["Automatisch getal", "Número automático", "Nombre automatique", "Automatische Zahl", "自動数値", "자동 숫자", "自動數字", "自动数字", "Автоматическое число", "Número automático"]),
    ("Whole number", ["Geheel getal", "Número entero", "Nombre entier", "Ganze Zahl", "整数", "정수", "整數", "整数", "Целое число", "Número inteiro"]),
    ("One decimal", ["Eén decimaal", "Un decimal", "Une décimale", "Eine Dezimalstelle", "小数第1位", "소수점 한 자리", "一位小數", "一位小数", "Один знак после запятой", "Uma casa decimal"]),
    ("Two decimals", ["Twee decimalen", "Dos decimales", "Deux décimales", "Zwei Dezimalstellen", "小数第2位", "소수점 두 자리", "兩位小數", "两位小数", "Два знака после запятой", "Duas casas decimais"]),
    ("Percentage", ["Percentage", "Porcentaje", "Pourcentage", "Prozentsatz", "パーセント", "백분율", "百分比", "百分比", "Процент", "Porcentagem"]),
    ("Short duration", ["Korte duur", "Duración corta", "Durée courte", "Kurze Dauer", "短い期間", "간단한 기간", "簡短時間", "简短时长", "Краткая длительность", "Duração curta"]),
    ("Detailed duration", ["Gedetailleerde duur", "Duración detallada", "Durée détaillée", "Detaillierte Dauer", "詳細な期間", "자세한 기간", "詳細時間", "详细时长", "Подробная длительность", "Duração detalhada"]),
    ("Usage and reset", ["Gebruik en reset", "Uso y restablecimiento", "Utilisation et réinitialisation", "Nutzung und Zurücksetzung", "使用量とリセット", "사용량 및 재설정", "用量與重設", "用量和重置", "Использование и сброс", "Uso e redefinição"]),
    ("Usage only", ["Alleen gebruik", "Solo uso", "Utilisation uniquement", "Nur Nutzung", "使用量のみ", "사용량만", "僅用量", "仅用量", "Только использование", "Somente uso"]),
    ("Plain text", ["Platte tekst", "Texto sin formato", "Texte brut", "Nur Text", "プレーンテキスト", "일반 텍스트", "純文字", "纯文本", "Обычный текст", "Texto simples"]),
    ("Provider values", ["Providerwaarden", "Valores del proveedor", "Valeurs du fournisseur", "Anbieterwerte", "プロバイダーの値", "제공자 값", "提供者值", "提供商值", "Значения провайдера", "Valores do provedor"]),
    ("Search values...", ["Waarden zoeken...", "Buscar valores...", "Rechercher des valeurs...", "Werte suchen...", "値を検索...", "값 검색...", "搜尋值...", "搜索值...", "Поиск значений...", "Pesquisar valores..."]),
    ("Format", ["Indeling", "Formato", "Format", "Format", "形式", "형식", "格式", "格式", "Формат", "Formato"]),
    ("Insert value", ["Waarde invoegen", "Insertar valor", "Insérer la valeur", "Wert einfügen", "値を挿入", "값 삽입", "插入值", "插入值", "Вставить значение", "Inserir valor"]),
    ("Guide", ["Handleiding", "Guía", "Guide", "Anleitung", "ガイド", "안내", "指南", "指南", "Руководство", "Guia"]),
    ("Type ordinary words directly in the editor.", ["Typ gewone woorden rechtstreeks in de editor.", "Escriba palabras normales directamente en el editor.", "Saisissez du texte ordinaire directement dans l’éditeur.", "Geben Sie normalen Text direkt im Editor ein.", "通常の文字はエディターに直接入力します。", "일반 텍스트는 편집기에 직접 입력하세요.", "直接在編輯器中輸入一般文字。", "直接在编辑器中输入普通文字。", "Введите обычный текст прямо в редакторе.", "Digite texto comum diretamente no editor."]),
    ("Select a provider value, choose its format, then insert it.", ["Selecteer een providerwaarde, kies de indeling en voeg deze in.", "Seleccione un valor del proveedor, elija su formato e insértelo.", "Sélectionnez une valeur du fournisseur, choisissez son format, puis insérez-la.", "Wählen Sie einen Anbieterwert und dessen Format aus und fügen Sie ihn ein.", "プロバイダーの値と形式を選び、挿入します。", "제공자 값과 형식을 선택한 다음 삽입하세요.", "選取提供者值與格式，然後插入。", "选择提供商值和格式，然后插入。", "Выберите значение провайдера и формат, затем вставьте его.", "Selecione um valor do provedor, escolha o formato e insira-o."]),
    ("Values are inserted at the end of the current text and can be moved or edited afterwards.", ["Waarden worden aan het einde van de huidige tekst ingevoegd en kunnen daarna worden verplaatst of bewerkt.", "Los valores se insertan al final del texto actual y después pueden moverse o editarse.", "Les valeurs sont insérées à la fin du texte actuel et peuvent ensuite être déplacées ou modifiées.", "Werte werden am Ende des aktuellen Textes eingefügt und können danach verschoben oder bearbeitet werden.", "値は現在のテキストの末尾に挿入され、後から移動または編集できます。", "값은 현재 텍스트 끝에 삽입되며 나중에 이동하거나 편집할 수 있습니다.", "值會插入目前文字的末尾，之後可移動或編輯。", "值会插入当前文本末尾，之后可移动或编辑。", "Значения вставляются в конец текста; затем их можно переместить или изменить.", "Os valores são inseridos no fim do texto atual e podem ser movidos ou editados depois."]),
    ("To show a literal opening brace, type:", ["Typ het volgende om een accolade openen weer te geven:", "Para mostrar una llave de apertura literal, escriba:", "Pour afficher une accolade ouvrante littérale, saisissez :", "Für eine öffnende geschweifte Klammer geben Sie Folgendes ein:", "開始波括弧をそのまま表示するには、次を入力します：", "여는 중괄호를 그대로 표시하려면 다음을 입력하세요:", "若要顯示左大括號字元，請輸入：", "要显示左大括号字符，请输入：", "Чтобы показать открывающую фигурную скобку, введите:", "Para mostrar uma chave de abertura literal, digite:"]),
    ("Advanced expressions are supported inside a value token.", ["Geavanceerde expressies worden binnen een waardetoken ondersteund.", "Se admiten expresiones avanzadas dentro de un token de valor.", "Les expressions avancées sont prises en charge dans un jeton de valeur.", "Erweiterte Ausdrücke werden innerhalb eines Wert-Tokens unterstützt.", "値トークン内では高度な式も使用できます。", "값 토큰 안에서 고급 표현식을 사용할 수 있습니다.", "值權杖內支援進階運算式。", "值标记内支持高级表达式。", "В токене значения поддерживаются расширенные выражения.", "Expressões avançadas são aceitas dentro de um token de valor."]),
    ("Variables", ["Variabelen", "Variables", "Variables", "Variablen", "変数", "변수", "變數", "变量", "Переменные", "Variáveis"]),
    ("Search variables...", ["Variabelen zoeken...", "Buscar variables...", "Rechercher des variables...", "Variablen suchen...", "変数を検索...", "변수 검색...", "搜尋變數...", "搜索变量...", "Поиск переменных...", "Pesquisar variáveis..."]),
    ("Constants", ["Constanten", "Constantes", "Constantes", "Konstanten", "定数", "상수", "常數", "常量", "Константы", "Constantes"]),
    ("Layout", ["Indeling", "Diseño", "Disposition", "Layout", "レイアウト", "레이아웃", "版面配置", "布局", "Макет", "Layout"]),
    ("Active provider", ["Actieve provider", "Proveedor activo", "Fournisseur actif", "Aktiver Anbieter", "アクティブなプロバイダー", "활성 제공자", "作用中的提供者", "活动提供商", "Активный провайдер", "Provedor ativo"]),
    ("Insert variable", ["Variabele invoegen", "Insertar variable", "Insérer la variable", "Variable einfügen", "変数を挿入", "변수 삽입", "插入變數", "插入变量", "Вставить переменную", "Inserir variável"]),
    ("Functions", ["Functies", "Funciones", "Fonctions", "Funktionen", "関数", "함수", "函式", "函数", "Функции", "Funções"]),
    ("Search functions...", ["Functies zoeken...", "Buscar funciones...", "Rechercher des fonctions...", "Funktionen suchen...", "関数を検索...", "함수 검색...", "搜尋函式...", "搜索函数...", "Поиск функций...", "Pesquisar funções..."]),
    ("Operators", ["Operatoren", "Operadores", "Opérateurs", "Operatoren", "演算子", "연산자", "運算子", "运算符", "Операторы", "Operadores"]),
    ("Insert operator", ["Operator invoegen", "Insertar operador", "Insérer l’opérateur", "Operator einfügen", "演算子を挿入", "연산자 삽입", "插入運算子", "插入运算符", "Вставить оператор", "Inserir operador"]),
    ("Text helper", ["Teksthulp", "Ayudante de texto", "Assistant de texte", "Texthilfe", "テキストヘルパー", "텍스트 도우미", "文字小幫手", "文本助手", "Помощник по тексту", "Assistente de texto"]),
    ("Build text from regular words and correctly formatted provider values.", ["Bouw tekst op uit gewone woorden en correct opgemaakte providerwaarden.", "Cree texto con palabras normales y valores del proveedor con el formato correcto.", "Créez du texte à partir de mots ordinaires et de valeurs de fournisseur correctement formatées.", "Erstellen Sie Text aus normalen Wörtern und korrekt formatierten Anbieterwerten.", "通常の文字と正しく書式設定されたプロバイダー値からテキストを作成します。", "일반 텍스트와 올바르게 형식화된 제공자 값으로 텍스트를 만드세요.", "使用一般文字與正確格式化的提供者值建立文字。", "使用普通文字和正确格式化的提供商值构建文本。", "Создайте текст из обычных слов и правильно отформатированных значений провайдера.", "Crie texto com palavras comuns e valores do provedor formatados corretamente."]),
    ("Discard text changes", ["Tekstwijzigingen negeren", "Descartar cambios de texto", "Ignorer les modifications du texte", "Textänderungen verwerfen", "テキストの変更を破棄", "텍스트 변경 내용 버리기", "捨棄文字變更", "放弃文本更改", "Отменить изменения текста", "Descartar alterações no texto"]),
    ("Close text helper", ["Teksthulp sluiten", "Cerrar el ayudante de texto", "Fermer l’assistant de texte", "Texthilfe schließen", "テキストヘルパーを閉じる", "텍스트 도우미 닫기", "關閉文字小幫手", "关闭文本助手", "Закрыть помощник по тексту", "Fechar assistente de texto"]),
    ("Type text here, then insert provider values below...", ["Typ hier tekst en voeg hieronder providerwaarden in...", "Escriba texto aquí y luego inserte los valores del proveedor abajo...", "Saisissez le texte ici, puis insérez les valeurs du fournisseur ci-dessous...", "Geben Sie hier Text ein und fügen Sie unten Anbieterwerte ein...", "ここにテキストを入力し、下からプロバイダー値を挿入します...", "여기에 텍스트를 입력한 다음 아래에서 제공자 값을 삽입하세요...", "在此輸入文字，然後從下方插入提供者值...", "在此输入文本，然后从下方插入提供商值...", "Введите текст здесь, затем добавьте значения провайдера ниже...", "Digite o texto aqui e insira os valores do provedor abaixo..."]),
    ("Live preview", ["Livevoorbeeld", "Vista previa en vivo", "Aperçu en direct", "Live-Vorschau", "ライブプレビュー", "실시간 미리보기", "即時預覽", "实时预览", "Предпросмотр", "Prévia ao vivo"]),
    ("Preview is empty", ["Voorbeeld is leeg", "La vista previa está vacía", "L’aperçu est vide", "Vorschau ist leer", "プレビューは空です", "미리보기가 비어 있습니다", "預覽是空的", "预览为空", "Предпросмотр пуст", "A prévia está vazia"]),
    ("Template is valid", ["Sjabloon is geldig", "La plantilla es válida", "Le modèle est valide", "Vorlage ist gültig", "テンプレートは有効です", "템플릿이 유효합니다", "範本有效", "模板有效", "Шаблон корректен", "O modelo é válido"]),
    ("Expression helper", ["Expressiehulp", "Ayudante de expresiones", "Assistant d’expression", "Ausdruckshilfe", "式ヘルパー", "표현식 도우미", "運算式小幫手", "表达式助手", "Помощник по выражениям", "Assistente de expressões"]),
    ("Build and validate an expression using the values supported by the theme engine.", ["Bouw en valideer een expressie met de waarden die door de thema-engine worden ondersteund.", "Cree y valide una expresión con los valores admitidos por el motor de temas.", "Créez et validez une expression avec les valeurs prises en charge par le moteur de thème.", "Erstellen und prüfen Sie einen Ausdruck mit den von der Theme-Engine unterstützten Werten.", "テーマエンジンが対応する値を使って式を作成し、検証します。", "테마 엔진에서 지원하는 값으로 표현식을 만들고 검증하세요.", "使用佈景主題引擎支援的值建立並驗證運算式。", "使用主题引擎支持的值构建并验证表达式。", "Создайте и проверьте выражение со значениями, поддерживаемыми движком тем.", "Crie e valide uma expressão usando os valores aceitos pelo mecanismo de temas."]),
    ("Discard expression changes", ["Expressiewijzigingen negeren", "Descartar cambios de la expresión", "Ignorer les modifications de l’expression", "Ausdrucksänderungen verwerfen", "式の変更を破棄", "표현식 변경 내용 버리기", "捨棄運算式變更", "放弃表达式更改", "Отменить изменения выражения", "Descartar alterações na expressão"]),
    ("Close expression helper", ["Expressiehulp sluiten", "Cerrar el ayudante de expresiones", "Fermer l’assistant d’expression", "Ausdruckshilfe schließen", "式ヘルパーを閉じる", "표현식 도우미 닫기", "關閉運算式小幫手", "关闭表达式助手", "Закрыть помощник по выражениям", "Fechar assistente de expressões"]),
    ("Enter an expression...", ["Voer een expressie in...", "Introduzca una expresión...", "Saisissez une expression...", "Ausdruck eingeben...", "式を入力...", "표현식 입력...", "輸入運算式...", "输入表达式...", "Введите выражение...", "Digite uma expressão..."]),
    ("Valid expression", ["Geldige expressie", "Expresión válida", "Expression valide", "Gültiger Ausdruck", "有効な式", "유효한 표현식", "有效的運算式", "有效表达式", "Корректное выражение", "Expressão válida"]),
    ("Current result", ["Huidig resultaat", "Resultado actual", "Résultat actuel", "Aktuelles Ergebnis", "現在の結果", "현재 결과", "目前結果", "当前结果", "Текущий результат", "Resultado atual"]),
    ("Smaller value", ["Kleinste waarde", "Valor menor", "Valeur la plus petite", "Kleinerer Wert", "小さい値", "더 작은 값", "較小的值", "较小的值", "Меньшее значение", "Menor valor"]),
    ("Larger value", ["Grootste waarde", "Valor mayor", "Valeur la plus grande", "Größerer Wert", "大きい値", "더 큰 값", "較大的值", "较大的值", "Большее значение", "Maior valor"]),
    ("Constrain a value", ["Waarde begrenzen", "Limitar un valor", "Limiter une valeur", "Wert begrenzen", "値を範囲内に制限", "값 범위 제한", "限制值的範圍", "限制值的范围", "Ограничить значение", "Limitar um valor"]),
    ("Nearest integer", ["Dichtstbijzijnde geheel getal", "Entero más cercano", "Entier le plus proche", "Nächste ganze Zahl", "最も近い整数", "가장 가까운 정수", "最接近的整數", "最接近的整数", "Ближайшее целое", "Inteiro mais próximo"]),
    ("Round down", ["Naar beneden afronden", "Redondear hacia abajo", "Arrondir à l’inférieur", "Abrunden", "切り捨て", "내림", "向下取整", "向下取整", "Округлить вниз", "Arredondar para baixo"]),
    ("Round up", ["Naar boven afronden", "Redondear hacia arriba", "Arrondir au supérieur", "Aufrunden", "切り上げ", "올림", "向上取整", "向上取整", "Округлить вверх", "Arredondar para cima"]),
    ("Absolute value", ["Absolute waarde", "Valor absoluto", "Valeur absolue", "Absolutwert", "絶対値", "절댓값", "絕對值", "绝对值", "Абсолютное значение", "Valor absoluto"]),
    ("Square root", ["Vierkantswortel", "Raíz cuadrada", "Racine carrée", "Quadratwurzel", "平方根", "제곱근", "平方根", "平方根", "Квадратный корень", "Raiz quadrada"]),
    ("Exponent", ["Exponent", "Exponente", "Exposant", "Exponent", "指数", "지수", "指數", "指数", "Степень", "Expoente"]),
    ("Conditional value", ["Voorwaardelijke waarde", "Valor condicional", "Valeur conditionnelle", "Bedingter Wert", "条件値", "조건부 값", "條件值", "条件值", "Условное значение", "Valor condicional"]),
    ("Linear interpolation", ["Lineaire interpolatie", "Interpolación lineal", "Interpolation linéaire", "Lineare Interpolation", "線形補間", "선형 보간", "線性插值", "线性插值", "Линейная интерполяция", "Interpolação linear"]),
    ("And", ["En", "Y", "Et", "Und", "かつ", "그리고", "且", "且", "И", "E"]),
    ("Or", ["Of", "O", "Ou", "Oder", "または", "또는", "或", "或", "ИЛИ", "Ou"]),
    ("Not", ["Niet", "No", "Non", "Nicht", "否定", "아님", "非", "非", "НЕ", "Não"]),
    ("Equal", ["Gelijk", "Igual", "Égal", "Gleich", "等しい", "같음", "等於", "等于", "Равно", "Igual"]),
    ("Not equal", ["Niet gelijk", "No igual", "Différent", "Ungleich", "等しくない", "같지 않음", "不等於", "不等于", "Не равно", "Diferente"]),
    ("Greater than", ["Groter dan", "Mayor que", "Supérieur à", "Größer als", "より大きい", "보다 큼", "大於", "大于", "Больше", "Maior que"]),
    ("Less than", ["Kleiner dan", "Menor que", "Inférieur à", "Kleiner als", "より小さい", "보다 작음", "小於", "小于", "Меньше", "Menor que"]),
    ("Greater or equal", ["Groter dan of gelijk aan", "Mayor o igual", "Supérieur ou égal", "Größer oder gleich", "以上", "크거나 같음", "大於或等於", "大于或等于", "Больше или равно", "Maior ou igual"]),
    ("Less or equal", ["Kleiner dan of gelijk aan", "Menor o igual", "Inférieur ou égal", "Kleiner oder gleich", "以下", "작거나 같음", "小於或等於", "小于或等于", "Меньше или равно", "Menor ou igual"]),
    ("Add", ["Optellen", "Sumar", "Additionner", "Addieren", "加算", "더하기", "加", "加", "Сложить", "Somar"]),
    ("Subtract", ["Aftrekken", "Restar", "Soustraire", "Subtrahieren", "減算", "빼기", "減", "减", "Вычесть", "Subtrair"]),
    ("Multiply", ["Vermenigvuldigen", "Multiplicar", "Multiplier", "Multiplizieren", "乗算", "곱하기", "乘", "乘", "Умножить", "Multiplicar"]),
    ("Divide", ["Delen", "Dividir", "Diviser", "Dividieren", "除算", "나누기", "除", "除", "Разделить", "Dividir"]),
    ("Remainder", ["Rest", "Resto", "Reste", "Rest", "剰余", "나머지", "餘數", "余数", "Остаток", "Resto"]),
    ("Grouping", ["Groepering", "Agrupación", "Regroupement", "Gruppierung", "グループ化", "그룹화", "群組", "分组", "Группировка", "Agrupamento"]),
    ("Enabled provider count", ["Aantal ingeschakelde providers", "Número de proveedores habilitados", "Nombre de fournisseurs activés", "Anzahl aktivierter Anbieter", "有効なプロバイダー数", "활성 제공자 수", "已啟用的提供者數", "已启用的提供商数", "Число включённых провайдеров", "Número de provedores habilitados"]),
    ("Session summary", ["Sessiesamenvatting", "Resumen de sesión", "Résumé de la session", "Sitzungsübersicht", "セッション概要", "세션 요약", "工作階段摘要", "会话摘要", "Сводка сеанса", "Resumo da sessão"]),
    ("Session used", ["Sessie gebruikt", "Sesión usada", "Session utilisée", "Sitzung genutzt", "セッション使用量", "세션 사용량", "工作階段已用", "会话已用", "Использовано за сеанс", "Sessão usada"]),
    ("Session remaining", ["Sessie resterend", "Sesión restante", "Session restante", "Sitzung verbleibend", "セッション残量", "세션 남은 양", "工作階段剩餘", "会话剩余", "Осталось в сеансе", "Sessão restante"]),
    ("Session reset", ["Sessiereset", "Restablecimiento de sesión", "Réinitialisation de la session", "Sitzungszurücksetzung", "セッションのリセット", "세션 재설정", "工作階段重設", "会话重置", "Сброс сеанса", "Redefinição da sessão"]),
    ("Weekly summary", ["Weeksamenvatting", "Resumen semanal", "Résumé hebdomadaire", "Wochenübersicht", "週間概要", "주간 요약", "每週摘要", "每周摘要", "Недельная сводка", "Resumo semanal"]),
    ("Weekly used", ["Week gebruikt", "Uso semanal", "Utilisation hebdomadaire", "Wöchentlich genutzt", "週間使用量", "주간 사용량", "每週已用", "每周已用", "Использовано за неделю", "Uso semanal"]),
    ("Weekly remaining", ["Week resterend", "Restante semanal", "Reste hebdomadaire", "Wöchentlich verbleibend", "週間残量", "주간 남은 양", "每週剩餘", "每周剩余", "Осталось на неделю", "Restante semanal"]),
    ("Weekly reset", ["Weekreset", "Restablecimiento semanal", "Réinitialisation hebdomadaire", "Wöchentliche Zurücksetzung", "週間リセット", "주간 재설정", "每週重設", "每周重置", "Недельный сброс", "Redefinição semanal"]),
    ("Labels", ["Labels", "Etiquetas", "Libellés", "Beschriftungen", "ラベル", "레이블", "標籤", "标签", "Метки", "Rótulos"]),
    ("Session window label", ["Label sessievenster", "Etiqueta de ventana de sesión", "Libellé de fenêtre de session", "Beschriftung des Sitzungsfensters", "セッション期間ラベル", "세션 기간 레이블", "工作階段視窗標籤", "会话窗口标签", "Метка окна сеанса", "Rótulo da janela de sessão"]),
    ("Weekly window label", ["Label weekvenster", "Etiqueta de ventana semanal", "Libellé de fenêtre hebdomadaire", "Beschriftung des Wochenfensters", "週間期間ラベル", "주간 기간 레이블", "每週視窗標籤", "每周窗口标签", "Метка недельного окна", "Rótulo da janela semanal"]),
    ("Now label", ["Label voor nu", "Etiqueta de ahora", "Libellé Maintenant", "Beschriftung für Jetzt", "「今」ラベル", "현재 레이블", "「現在」標籤", "“现在”标签", "Метка «Сейчас»", "Rótulo de agora"]),
    ("Action helper", ["Actiehulp", "Ayudante de acciones", "Assistant d’action", "Aktionshilfe", "アクションヘルパー", "작업 도우미", "動作小幫手", "操作助手", "Помощник по действиям", "Assistente de ações"]),
    ("Build safe mouse actions that affect layers at runtime.", ["Bouw veilige muisacties die lagen tijdens runtime beïnvloeden.", "Cree acciones de ratón seguras que afecten a las capas en tiempo de ejecución.", "Créez des actions de souris sûres qui affectent les calques à l’exécution.", "Erstellen Sie sichere Mausaktionen, die Ebenen zur Laufzeit beeinflussen.", "実行時にレイヤーへ作用する安全なマウスアクションを作成します。", "실행 중 레이어에 영향을 주는 안전한 마우스 작업을 만드세요.", "建立在執行階段影響圖層的安全滑鼠動作。", "构建在运行时影响图层的安全鼠标操作。", "Создавайте безопасные действия мыши, влияющие на слои во время выполнения.", "Crie ações de mouse seguras que afetem camadas durante a execução."]),
    ("Choose one action for this context menu item.", ["Kies één actie voor dit contextmenu-item.", "Elija una acción para este elemento del menú contextual.", "Choisissez une action pour cet élément du menu contextuel.", "Wählen Sie eine Aktion für diesen Kontextmenüeintrag.", "このコンテキストメニュー項目のアクションを1つ選択します。", "이 컨텍스트 메뉴 항목에 사용할 작업 하나를 선택하세요.", "為此快顯功能表項目選擇一個動作。", "为此上下文菜单项选择一个操作。", "Выберите одно действие для этого пункта контекстного меню.", "Escolha uma ação para este item do menu de contexto."]),
    ("Discard action changes", ["Actiewijzigingen negeren", "Descartar cambios de acción", "Ignorer les modifications de l’action", "Aktionsänderungen verwerfen", "アクションの変更を破棄", "작업 변경 내용 버리기", "捨棄動作變更", "放弃操作更改", "Отменить изменения действия", "Descartar alterações da ação"]),
    ("Close action helper", ["Actiehulp sluiten", "Cerrar el ayudante de acciones", "Fermer l’assistant d’action", "Aktionshilfe schließen", "アクションヘルパーを閉じる", "작업 도우미 닫기", "關閉動作小幫手", "关闭操作助手", "Закрыть помощник по действиям", "Fechar assistente de ações"]),
    ("Enter actions...", ["Voer acties in...", "Introduzca acciones...", "Saisissez des actions...", "Aktionen eingeben...", "アクションを入力...", "작업 입력...", "輸入動作...", "输入操作...", "Введите действия...", "Digite as ações..."]),
    ("Valid actions", ["Geldige acties", "Acciones válidas", "Actions valides", "Gültige Aktionen", "有効なアクション", "유효한 작업", "有效的動作", "有效操作", "Корректные действия", "Ações válidas"]),
    ("actions", ["acties", "acciones", "actions", "Aktionen", "アクション", "작업", "個動作", "个操作", "действий", "ações"]),
    ("One menu action", ["Eén menuactie", "Una acción de menú", "Une action de menu", "Eine Menüaktion", "メニューアクション1件", "메뉴 작업 1개", "一個選單動作", "一个菜单操作", "Одно действие меню", "Uma ação de menu"]),
    ("Actions", ["Acties", "Acciones", "Actions", "Aktionen", "アクション", "작업", "動作", "操作", "Действия", "Ações"]),
    ("Show dashboard", ["Dashboard tonen", "Mostrar panel", "Afficher le tableau de bord", "Dashboard anzeigen", "ダッシュボードを表示", "대시보드 표시", "顯示儀表板", "显示仪表板", "Показать панель", "Mostrar painel"]),
    ("Toggle dashboard", ["Dashboard omschakelen", "Alternar panel", "Basculer le tableau de bord", "Dashboard umschalten", "ダッシュボードを切り替え", "대시보드 전환", "切換儀表板", "切换仪表板", "Переключить панель", "Alternar painel"]),
    ("Context menu", ["Contextmenu", "Menú contextual", "Menu contextuel", "Kontextmenü", "コンテキストメニュー", "컨텍스트 메뉴", "快顯功能表", "上下文菜单", "Контекстное меню", "Menu de contexto"]),
    ("Show context menu", ["Contextmenu tonen", "Mostrar menú contextual", "Afficher le menu contextuel", "Kontextmenü anzeigen", "コンテキストメニューを表示", "컨텍스트 메뉴 표시", "顯示快顯功能表", "显示上下文菜单", "Показать контекстное меню", "Mostrar menu de contexto"]),
    ("Set property", ["Eigenschap instellen", "Establecer propiedad", "Définir la propriété", "Eigenschaft festlegen", "プロパティを設定", "속성 설정", "設定屬性", "设置属性", "Задать свойство", "Definir propriedade"]),
    ("Toggle property", ["Eigenschap omschakelen", "Alternar propiedad", "Basculer la propriété", "Eigenschaft umschalten", "プロパティを切り替え", "속성 전환", "切換屬性", "切换属性", "Переключить свойство", "Alternar propriedade"]),
    ("Toggle currently supports Render only", ["Omschakelen ondersteunt momenteel alleen Render", "Alternar solo admite Render actualmente", "Le basculement ne prend actuellement en charge que Rendu", "Umschalten unterstützt derzeit nur Rendern", "切り替えは現在レンダーのみ対応しています", "전환은 현재 렌더링만 지원합니다", "切換目前僅支援轉譯", "切换目前仅支持渲染", "Переключение пока поддерживает только отрисовку", "Alternar aceita apenas Renderizar no momento"]),
    ("Reset property", ["Eigenschap herstellen", "Restablecer propiedad", "Réinitialiser la propriété", "Eigenschaft zurücksetzen", "プロパティをリセット", "속성 재설정", "重設屬性", "重置属性", "Сбросить свойство", "Redefinir propriedade"]),
    ("Increase value", ["Waarde verhogen", "Aumentar valor", "Augmenter la valeur", "Wert erhöhen", "値を増やす", "값 증가", "增加值", "增加值", "Увеличить значение", "Aumentar valor"]),
    ("Choose a numeric property", ["Kies een numerieke eigenschap", "Elija una propiedad numérica", "Choisissez une propriété numérique", "Wählen Sie eine numerische Eigenschaft", "数値プロパティを選択してください", "숫자 속성을 선택하세요", "請選擇數值屬性", "请选择数值属性", "Выберите числовое свойство", "Escolha uma propriedade numérica"]),
    ("Decrease value", ["Waarde verlagen", "Disminuir valor", "Diminuer la valeur", "Wert verringern", "値を減らす", "값 감소", "減少值", "减小值", "Уменьшить значение", "Diminuir valor"]),
    ("Actions run from top to bottom in one update.", ["Acties worden van boven naar beneden in één update uitgevoerd.", "Las acciones se ejecutan de arriba abajo en una actualización.", "Les actions s’exécutent de haut en bas en une seule mise à jour.", "Aktionen werden in einer Aktualisierung von oben nach unten ausgeführt.", "アクションは1回の更新で上から順に実行されます。", "작업은 한 번의 업데이트에서 위에서 아래로 실행됩니다.", "動作會在一次更新中由上而下執行。", "操作会在一次更新中从上到下执行。", "Действия выполняются сверху вниз за одно обновление.", "As ações são executadas de cima para baixo em uma atualização."]),
    ("Layers", ["Lagen", "Capas", "Calques", "Ebenen", "レイヤー", "레이어", "圖層", "图层", "Слои", "Camadas"]),
    ("Self", ["Zelf", "Actual", "Soi-même", "Selbst", "自身", "자신", "自身", "自身", "Текущий слой", "Próprio"]),
    ("Properties", ["Eigenschappen", "Propiedades", "Propriétés", "Eigenschaften", "プロパティ", "속성", "屬性", "属性", "Свойства", "Propriedades"]),
    ("Value expression", ["Waarde-expressie", "Expresión de valor", "Expression de valeur", "Wertausdruck", "値の式", "값 표현식", "值運算式", "值表达式", "Выражение значения", "Expressão de valor"]),
    ("e.g. false, 120, parent.width / 2", ["bijv. false, 120, parent.width / 2", "p. ej., false, 120, parent.width / 2", "p. ex. false, 120, parent.width / 2", "z. B. false, 120, parent.width / 2", "例: false、120、parent.width / 2", "예: false, 120, parent.width / 2", "例如 false、120、parent.width / 2", "例如 false、120、parent.width / 2", "например false, 120, parent.width / 2", "ex.: false, 120, parent.width / 2"]),
    ("Reset removes the runtime override and restores the saved expression.", ["Herstellen verwijdert de runtime-overschrijving en zet de opgeslagen expressie terug.", "Restablecer elimina la anulación en tiempo de ejecución y restaura la expresión guardada.", "Réinitialiser supprime le remplacement d’exécution et restaure l’expression enregistrée.", "Zurücksetzen entfernt die Laufzeitüberschreibung und stellt den gespeicherten Ausdruck wieder her.", "リセットすると実行時の上書きが削除され、保存済みの式に戻ります。", "재설정하면 런타임 재정의가 제거되고 저장된 표현식이 복원됩니다.", "重設會移除執行階段覆寫並還原已儲存的運算式。", "重置会移除运行时覆盖并恢复已保存的表达式。", "Сброс удаляет переопределение времени выполнения и восстанавливает сохранённое выражение.", "Redefinir remove a substituição de execução e restaura a expressão salva."]),
    ("Layer actions", ["Laagacties", "Acciones de capa", "Actions de calque", "Ebenenaktionen", "レイヤーアクション", "레이어 작업", "圖層動作", "图层操作", "Действия слоя", "Ações de camada"]),
    ("Options", ["Opties", "Opciones", "Options", "Optionen", "オプション", "옵션", "選項", "选项", "Параметры", "Opções"]),
    ("Update frequency", ["Updatefrequentie", "Frecuencia de actualización", "Fréquence de mise à jour", "Aktualisierungshäufigkeit", "更新頻度", "업데이트 빈도", "更新頻率", "更新频率", "Частота обновления", "Frequência de atualização"]),
    ("Provider", ["Provider", "Proveedor", "Fournisseur", "Anbieter", "プロバイダー", "제공자", "提供者", "提供商", "Провайдер", "Provedor"]),
    ("Edit quoted values in the action field for URLs, languages, and layer-action scripts.", ["Bewerk waarden tussen aanhalingstekens in het actieveld voor URL's, talen en laagactiescripts.", "Edite los valores entre comillas del campo de acción para URL, idiomas y scripts de acciones de capa.", "Modifiez les valeurs entre guillemets du champ d’action pour les URL, les langues et les scripts d’action de calque.", "Bearbeiten Sie in Anführungszeichen gesetzte Werte im Aktionsfeld für URLs, Sprachen und Ebenenaktionsskripte.", "URL、言語、レイヤーアクションスクリプトは、アクション欄の引用符内を編集します。", "URL, 언어 및 레이어 작업 스크립트는 작업 필드의 따옴표 안 값을 편집하세요.", "請在動作欄位中編輯 URL、語言及圖層動作指令碼的引號值。", "请在操作字段中编辑 URL、语言和图层操作脚本的引号值。", "Изменяйте значения в кавычках в поле действия для URL, языков и сценариев действий слоя.", "Edite os valores entre aspas no campo de ação para URLs, idiomas e scripts de ação de camada."]),
    ("Set update frequency", ["Updatefrequentie instellen", "Establecer frecuencia de actualización", "Définir la fréquence de mise à jour", "Aktualisierungshäufigkeit festlegen", "更新頻度を設定", "업데이트 빈도 설정", "設定更新頻率", "设置更新频率", "Задать частоту обновления", "Definir frequência de atualização"]),
    ("Toggle provider", ["Provider omschakelen", "Alternar proveedor", "Basculer le fournisseur", "Anbieter umschalten", "プロバイダーを切り替え", "제공자 전환", "切換提供者", "切换提供商", "Переключить провайдера", "Alternar provedor"]),
    ("Toggle Start with Windows", ["Starten met Windows omschakelen", "Alternar Iniciar con Windows", "Basculer le démarrage avec Windows", "Mit Windows starten umschalten", "Windowsと同時起動を切り替え", "Windows 시작 전환", "切換隨 Windows 啟動", "切换随 Windows 启动", "Переключить запуск с Windows", "Alternar Iniciar com o Windows"]),
    ("Set language", ["Taal instellen", "Establecer idioma", "Définir la langue", "Sprache festlegen", "言語を設定", "언어 설정", "設定語言", "设置语言", "Задать язык", "Definir idioma"]),
    ("Toggle layer Render", ["Renderen van laag omschakelen", "Alternar renderizado de capa", "Basculer le rendu du calque", "Rendern der Ebene umschalten", "レイヤーのレンダーを切り替え", "레이어 렌더링 전환", "切換圖層轉譯", "切换图层渲染", "Переключить отрисовку слоя", "Alternar renderização da camada"]),
    ("Run layer actions", ["Laagacties uitvoeren", "Ejecutar acciones de capa", "Exécuter les actions de calque", "Ebenenaktionen ausführen", "レイヤーアクションを実行", "레이어 작업 실행", "執行圖層動作", "运行图层操作", "Выполнить действия слоя", "Executar ações de camada"]),
    ("Open URL", ["URL openen", "Abrir URL", "Ouvrir l’URL", "URL öffnen", "URLを開く", "URL 열기", "開啟 URL", "打开 URL", "Открыть URL", "Abrir URL"]),
    ("Show widget", ["Widget tonen", "Mostrar widget", "Afficher le widget", "Widget anzeigen", "ウィジェットを表示", "위젯 표시", "顯示小工具", "显示小组件", "Показать виджет", "Mostrar widget"]),
    ("Save changes?", ["Wijzigingen opslaan?", "¿Guardar cambios?", "Enregistrer les modifications ?", "Änderungen speichern?", "変更を保存しますか？", "변경 내용을 저장할까요?", "要儲存變更嗎？", "是否保存更改？", "Сохранить изменения?", "Salvar alterações?"]),
    ("Save and continue", ["Opslaan en doorgaan", "Guardar y continuar", "Enregistrer et continuer", "Speichern und fortfahren", "保存して続行", "저장하고 계속", "儲存並繼續", "保存并继续", "Сохранить и продолжить", "Salvar e continuar"]),
    ("New theme", ["Nieuw thema", "Tema nuevo", "Nouveau thème", "Neues Theme", "新しいテーマ", "새 테마", "新增佈景主題", "新建主题", "Новая тема", "Novo tema"]),
    ("Name the new theme", ["Geef het nieuwe thema een naam", "Asigne un nombre al tema nuevo", "Nommez le nouveau thème", "Benennen Sie das neue Theme", "新しいテーマに名前を付けます", "새 테마의 이름을 지정하세요", "為新佈景主題命名", "为新主题命名", "Назовите новую тему", "Dê um nome ao novo tema"]),
    ("Theme name", ["Themanaam", "Nombre del tema", "Nom du thème", "Theme-Name", "テーマ名", "테마 이름", "佈景主題名稱", "主题名称", "Название темы", "Nome do tema"]),
    ("Duplicate theme", ["Thema dupliceren", "Duplicar tema", "Dupliquer le thème", "Theme duplizieren", "テーマを複製", "테마 복제", "複製佈景主題", "复制主题", "Дублировать тему", "Duplicar tema"]),
    ("Name the editable copy", ["Geef de bewerkbare kopie een naam", "Asigne un nombre a la copia editable", "Nommez la copie modifiable", "Benennen Sie die bearbeitbare Kopie", "編集可能なコピーに名前を付けます", "편집 가능한 복사본의 이름을 지정하세요", "為可編輯的副本命名", "为可编辑副本命名", "Назовите редактируемую копию", "Dê um nome à cópia editável"]),
    ("Create copy", ["Kopie maken", "Crear copia", "Créer une copie", "Kopie erstellen", "コピーを作成", "복사본 만들기", "建立副本", "创建副本", "Создать копию", "Criar cópia"]),
    ("Delete theme?", ["Thema verwijderen?", "¿Eliminar tema?", "Supprimer le thème ?", "Theme löschen?", "テーマを削除しますか？", "테마를 삭제할까요?", "要刪除佈景主題嗎？", "是否删除主题？", "Удалить тему?", "Excluir tema?"]),
    ("Are you sure you want to delete {name}?", ["Weet u zeker dat u {name} wilt verwijderen?", "¿Seguro que desea eliminar {name}?", "Voulez-vous vraiment supprimer {name} ?", "Möchten Sie {name} wirklich löschen?", "{name} を削除してもよろしいですか？", "{name}을(를) 삭제하시겠습니까?", "確定要刪除 {name} 嗎？", "确定要删除 {name} 吗？", "Удалить {name}?", "Tem certeza de que deseja excluir {name}?"]),
    ("Delete context menu?", ["Contextmenu verwijderen?", "¿Eliminar menú contextual?", "Supprimer le menu contextuel ?", "Kontextmenü löschen?", "コンテキストメニューを削除しますか？", "컨텍스트 메뉴를 삭제할까요?", "要刪除快顯功能表嗎？", "是否删除上下文菜单？", "Удалить контекстное меню?", "Excluir menu de contexto?"]),
    ("Delete context menu", ["Contextmenu verwijderen", "Eliminar menú contextual", "Supprimer le menu contextuel", "Kontextmenü löschen", "コンテキストメニューを削除", "컨텍스트 메뉴 삭제", "刪除快顯功能表", "删除上下文菜单", "Удалить контекстное меню", "Excluir menu de contexto"]),
    ("Delete asset?", ["Asset verwijderen?", "¿Eliminar recurso?", "Supprimer la ressource ?", "Asset löschen?", "アセットを削除しますか？", "에셋을 삭제할까요?", "要刪除資產嗎？", "是否删除资源？", "Удалить ресурс?", "Excluir recurso?"]),
    ("Are you sure you want to delete {name} from the asset library and all themes using it?", ["Weet u zeker dat u {name} uit de assetbibliotheek en alle thema's die het gebruiken wilt verwijderen?", "¿Seguro que desea eliminar {name} de la biblioteca de recursos y de todos los temas que lo usan?", "Voulez-vous vraiment supprimer {name} de la bibliothèque de ressources et de tous les thèmes qui l’utilisent ?", "Möchten Sie {name} wirklich aus der Asset-Bibliothek und allen verwendenden Themes löschen?", "{name} をアセットライブラリと使用中のすべてのテーマから削除してもよろしいですか？", "{name}을(를) 에셋 라이브러리와 이를 사용하는 모든 테마에서 삭제하시겠습니까?", "確定要從資產庫及所有使用它的佈景主題中刪除 {name} 嗎？", "确定要从资源库及所有使用它的主题中删除 {name} 吗？", "Удалить {name} из библиотеки ресурсов и всех использующих его тем?", "Tem certeza de que deseja excluir {name} da biblioteca de recursos e de todos os temas que o utilizam?"]),
];

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn every_helper_key_has_a_translation_for_each_locale() {
        let mut keys = HashSet::new();
        for (key, translations) in TRANSLATIONS {
            assert!(
                keys.insert(*key),
                "duplicate helper translation key {key:?}"
            );
            for translation in translations {
                assert!(
                    !translation.trim().is_empty(),
                    "empty translation for {key:?}"
                );
            }
        }
    }
}
