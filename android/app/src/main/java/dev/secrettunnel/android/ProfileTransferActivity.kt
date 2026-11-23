package dev.secrettunnel.android

import android.app.Activity
import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.ActivityNotFoundException
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.graphics.Bitmap
import android.graphics.Typeface
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.text.Editable
import android.text.InputType
import android.text.TextWatcher
import android.view.Gravity
import android.view.View
import android.view.WindowManager
import android.view.inputmethod.EditorInfo
import android.widget.*
import androidx.core.content.FileProvider
import androidx.core.view.WindowCompat
import com.google.zxing.BarcodeFormat
import com.google.zxing.EncodeHintType
import com.google.zxing.integration.android.IntentIntegrator
import com.google.zxing.qrcode.decoder.ErrorCorrectionLevel
import com.journeyapps.barcodescanner.BarcodeEncoder
import org.json.JSONObject
import java.io.File

internal const val MAX_TRANSFER_BYTES = 1_048_576
internal class ProfileTransferSession(private val context: Context) {
    var exportIds:List<String>?=null
    var payload=""
    var code=""
    var preview:JSONObject?=null
    var exported:JSONObject?=null
    var qr:Bitmap?=null
    var done=false
    var error=""
    var notice=""
    @Volatile var busy=false
        private set
    var changed:(()->Unit)?=null
    private val main=Handler(Looper.getMainLooper())
    private fun notifyChange(){main.post{changed?.invoke()}}
    fun work(action:()->Unit){
        if(busy)return
        busy=true;error="";notice="";notifyChange()
        Thread{
            try{NativeBridge.requireAvailable();action()}
            catch(error:Exception){this.error=error.message?:"Не удалось перенести серверы"}
            finally{busy=false;notifyChange()}
        }.start()
    }
    private fun response(raw:String)=JSONObject(raw).also{require(!it.has("error")){it.optString("error")}}
    fun prepareExport(ids:List<String>)=work{
        val document=SettingsStore(context).transferDocument(ids)
        val result=response(NativeBridge.transferExport(document.toString()))
        qr=if(result.optBoolean("qr_available"))BarcodeEncoder().encodeBitmap(result.getString("link"),BarcodeFormat.QR_CODE,800,800,
            mapOf(EncodeHintType.ERROR_CORRECTION to ErrorCorrectionLevel.M,EncodeHintType.MARGIN to 4))else null
        exported=result
    }
    fun unlock()=work{preview=response(NativeBridge.transferUnlock(payload,code,true))}
    fun importProfiles()=work{
        require(!TunnelVpnService.active){"Сначала остановите туннель"}
        val document=response(NativeBridge.transferUnlock(payload,code,false))
        SettingsStore(context).importTransferred(document)
        payload="";code="";preview=null;done=true
    }
    fun clear(){payload="";code="";exported=null;preview=null;qr=null}
}

class ProfileTransferActivity:Activity(){
    private lateinit var session:ProfileTransferSession
    private lateinit var palette:Palette
    private lateinit var body:LinearLayout
    private val tv get()=BuildConfig.FLAVOR=="tv" || resources.configuration.uiMode and Configuration.UI_MODE_TYPE_MASK == Configuration.UI_MODE_TYPE_TELEVISION
    private var showCode=false
    private val density get()=resources.displayMetrics.density
    private fun dp(value:Int)=(value*density).toInt()
    override fun onCreate(state:Bundle?){
        val dark=SettingsStore(this).appearance.isDark(resources.configuration)
        setTheme(if(dark)R.style.Theme_SecretTunnel_Dark else R.style.Theme_SecretTunnel_Light)
        super.onCreate(state)
        window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        palette=if(dark)Palette.dark else Palette.light
        window.statusBarColor=palette.background;window.navigationBarColor=palette.background
        WindowCompat.getInsetsController(window,window.decorView).apply{isAppearanceLightStatusBars=!dark;isAppearanceLightNavigationBars=!dark}
        session=lastNonConfigurationInstance as? ProfileTransferSession ?: ProfileTransferSession(applicationContext).also{model->
            if(intent.action==Intent.ACTION_VIEW){model.payload=intent.dataString.orEmpty().take(MAX_TRANSFER_BYTES+1)}
            else model.exportIds=intent.getStringArrayListExtra("server_ids")?.toList()
        }
        session.changed={if(!isFinishing&&!isDestroyed)render()};render()
    }
    @Deprecated("Retained transfer session") override fun onRetainNonConfigurationInstance():Any=session
    override fun onDestroy(){session.changed=null;if(isFinishing)session.clear();super.onDestroy()}
    @Deprecated("Platform back") override fun onBackPressed(){if(!session.busy)finish()}

    private fun render(){
        val scroll=ScrollView(this).apply{isFillViewport=true;setBackgroundColor(palette.background)}
        body=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL;setPadding(dp(20),dp(24),dp(20),dp(24))}
        val frame=FrameLayout(this);frame.addView(body,FrameLayout.LayoutParams(if(tv)dp(minOf(640,resources.configuration.screenWidthDp))else -1,-2,Gravity.TOP or Gravity.CENTER_HORIZONTAL));scroll.addView(frame)
        setContentView(scroll)
        text(if(session.exportIds==null)"Импортировать серверы" else "Поделиться серверами",24,true)
        if(session.exportIds!=null)exportUi()else importUi()
        if(session.busy){body.addView(ProgressBar(this));text("Обрабатываем настройки…")}
        if(session.error.isNotEmpty())text(session.error,14,false,palette.error)
        if(session.notice.isNotEmpty())text(session.notice,14,false,palette.muted)
        button("Закрыть"){finish()}
    }
    private fun importUi(){
        if(session.done){
            setResult(RESULT_OK)
            text("Серверы добавлены",20,true);text("Существующие подключения сохранены. Выберите сервер и проверьте соединение.")
            button("Готово",true){setResult(RESULT_OK);finish()};return
        }
        val preview=session.preview
        if(preview!=null){
            text("Будут добавлены серверы",20,true)
            val servers=preview.getJSONArray("servers")
            for(index in 0 until servers.length()){
                val server=servers.getJSONObject(index)
                text(server.getString("name"),18,true)
                text("${server.getString("user")}@${server.getString("host")}:${server.getInt("port")}")
                if(!server.isNull("jump_id")){
                    val parent=(0 until servers.length()).map{servers.getJSONObject(it)}.first{it.getString("id")==server.getString("jump_id")}
                    text("Через ${parent.getString("name")}")
                }
            }
            text("Добавляйте только настройки от доверенного отправителя. Существующие серверы не будут заменены.")
            button("Добавить серверы",true){session.importProfiles()}
            button("Назад"){session.preview=null;render()};return
        }
        text("Вставьте ссылку или откройте файл .stprofile. Код переноса получите у отправителя отдельно.")
        if(!tv&&packageManager.hasSystemFeature(PackageManager.FEATURE_CAMERA_ANY))button("Сканировать QR-код",true){
            IntentIntegrator(this).setDesiredBarcodeFormats(IntentIntegrator.QR_CODE).setPrompt("Наведите камеру на QR-код Secret Tunnel")
                .setBeepEnabled(false).setOrientationLocked(false).initiateScan()
        }
        button("Открыть файл"){
            try{startActivityForResult(Intent(Intent.ACTION_OPEN_DOCUMENT).setType("*/*").addCategory(Intent.CATEGORY_OPENABLE),201)}
            catch(_:ActivityNotFoundException){
                try{startActivityForResult(Intent(Intent.ACTION_GET_CONTENT).setType("*/*").addCategory(Intent.CATEGORY_OPENABLE),201)}
                catch(_:ActivityNotFoundException){error("На устройстве нет выбора файлов. Установите файловый менеджер или вставьте ссылку переноса.")}
            }
        }
        field("Ссылка переноса",session.payload,2101,false){session.payload=it}
        field("Код переноса",session.code,2102,true){session.code=it}
        button("Расшифровать и проверить",true){session.unlock()}
    }
    private fun exportUi(){
        val exported=session.exported
        if(exported==null){
            text("Подключение включает ключ доступа. Получатель сможет войти на ваш сервер.")
            text("Промежуточные серверы цепочки добавятся автоматически. Код переноса передайте отдельно.")
            button("Подготовить перенос",true){session.prepareExport(requireNotNull(session.exportIds))};return
        }
        text("Серверов в переносе: ${exported.getInt("server_count")}")
        session.qr?.let{bitmap->
            body.addView(ImageView(this).apply{setImageBitmap(bitmap);contentDescription="Зашифрованные настройки серверов";setBackgroundColor(android.graphics.Color.WHITE)},LinearLayout.LayoutParams(dp(300),dp(300)).apply{gravity=Gravity.CENTER_HORIZONTAL;topMargin=dp(16)})
        }?:text("Настройки слишком велики для QR-кода. Передайте ссылку или файл.")
        button("Скопировать ссылку"){copy(exported.getString("link"),"Ссылка переноса")}
        button("Поделиться ссылкой"){
            startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT,exported.getString("link")),"Передать зашифрованные настройки"))
        }
        button("Сохранить файл",true){startActivityForResult(Intent(Intent.ACTION_CREATE_DOCUMENT).setType("application/octet-stream").addCategory(Intent.CATEGORY_OPENABLE).putExtra(Intent.EXTRA_TITLE,"SecretTunnel-servers.stprofile"),202)}
        button("Поделиться файлом"){
            val directory=File(cacheDir,"profile-transfers").apply{mkdirs()}
            val file=File(directory,"SecretTunnel-servers.stprofile");file.writeText(exported.getString("link"))
            val uri=FileProvider.getUriForFile(this,"${packageName}.updates",file)
            startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).setType("application/octet-stream").putExtra(Intent.EXTRA_STREAM,uri)
                .apply{clipData=ClipData.newUri(contentResolver,"Зашифрованные серверы",uri)}.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION),"Передать зашифрованные настройки"))
        }
        text("Код переноса",18,true)
        text("Сохраните его отдельно от ссылки или файла. После закрытия окна код этого переноса нельзя восстановить.")
        if(showCode)text(exported.getString("code"),20,true).apply{typeface=Typeface.MONOSPACE;setTextIsSelectable(true);isSaveEnabled=false}
        button(if(showCode)"Скрыть код" else "Показать код"){showCode=!showCode;render()}
        button("Скопировать код"){copy(exported.getString("code"),"Код переноса")}
    }
    private fun copy(value:String,label:String){
        val clip=ClipData.newPlainText(label,value)
        if(android.os.Build.VERSION.SDK_INT>=33)clip.description.extras=android.os.PersistableBundle().apply{putBoolean(ClipDescription.EXTRA_IS_SENSITIVE,true)}
        (getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager).setPrimaryClip(clip)
        Toast.makeText(this,"Скопировано",Toast.LENGTH_SHORT).show()
    }
    private fun text(value:String,size:Int=14,bold:Boolean=false,color:Int=palette.text):TextView=TextView(this).apply{
        text=value;textSize=size.toFloat();setTextColor(color);if(bold)typeface=Typeface.create("sans-serif-medium",Typeface.NORMAL)
        body.addView(this,LinearLayout.LayoutParams(-1,-2).apply{topMargin=dp(12)})
    }
    private fun button(title:String,primary:Boolean=false,action:()->Unit){
        body.addView(Button(this).apply{
            text=title;isAllCaps=false;minimumHeight=dp(50);setTextColor(if(primary)palette.onAccent else palette.accent)
            background=palette.panel(if(primary)palette.accent else palette.raised,dp(14).toFloat());isEnabled=!session.busy
            setOnClickListener{try{action()}catch(error:Exception){session.error=error.message?:"Операция недоступна";render()}}
        },LinearLayout.LayoutParams(-1,-2).apply{topMargin=dp(12)})
    }
    private fun field(title:String,value:String,idValue:Int,code:Boolean,changed:(String)->Unit){
        text(title,13,true,palette.muted)
        body.addView(EditText(this).apply{
            id=idValue;setText(value);setTextColor(palette.text);isSaveEnabled=false;importantForAutofill=View.IMPORTANT_FOR_AUTOFILL_NO
            inputType=InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS or if(code)InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD else InputType.TYPE_TEXT_FLAG_MULTI_LINE
            imeOptions=imeOptions or EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING
            setSingleLine(code);maxLines=if(code)1 else 4;isEnabled=!session.busy
            filters=arrayOf(android.text.InputFilter.LengthFilter(if(code)128 else MAX_TRANSFER_BYTES))
            background=palette.panel(palette.surface,dp(12).toFloat());setPadding(dp(12),dp(12),dp(12),dp(12))
            addTextChangedListener(object:TextWatcher{override fun beforeTextChanged(s:CharSequence?,start:Int,count:Int,after:Int){};override fun onTextChanged(s:CharSequence?,start:Int,before:Int,count:Int){changed(s.toString())};override fun afterTextChanged(s:Editable?){} })
        },LinearLayout.LayoutParams(-1,-2).apply{topMargin=dp(8)})
    }
    @Deprecated("Platform activity result") override fun onActivityResult(requestCode:Int,resultCode:Int,data:Intent?){
        super.onActivityResult(requestCode,resultCode,data)
        val scanned=IntentIntegrator.parseActivityResult(requestCode,resultCode,data)
        if(scanned!=null){if(scanned.contents!=null){session.payload=scanned.contents.take(MAX_TRANSFER_BYTES+1);session.preview=null;render()};return}
        if(resultCode!=RESULT_OK)return
        val uri=data?.data?:return
        if(requestCode==201)session.work{
            val bytes=contentResolver.openInputStream(uri)?.use{stream->
                val output=java.io.ByteArrayOutputStream();val block=ByteArray(8192)
                while(output.size()<=MAX_TRANSFER_BYTES){val count=stream.read(block);if(count<0)break;output.write(block,0,count)}
                output.toByteArray()
            }?:error("Не удалось открыть файл")
            require(bytes.size<=MAX_TRANSFER_BYTES){"Файл больше 1 МБ"}
            session.payload=bytes.toString(Charsets.UTF_8);session.preview=null
        }
        if(requestCode==202)session.work{
            val encrypted=requireNotNull(session.exported).getString("link")
            contentResolver.openOutputStream(uri,"wt")?.use{it.write(encrypted.toByteArray(Charsets.UTF_8))}?:error("Не удалось сохранить файл")
            session.notice="Зашифрованный файл сохранён"
        }
    }
}
