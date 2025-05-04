function updateBanDisplay(reason, seconds) {
    showBan(reason, seconds);
}

function updateBanTimer(seconds) {
    disableInput(seconds);
}

function clearBanDisplay() {
    if (currentBanNotification) {
        currentBanNotification.remove();
        currentBanNotification = null;
    }
    enableInput();
}

function showAuthForms() {
    document.getElementById('auth-forms').style.display = 'block';
    document.getElementById('message-input-container').style.display = 'none';
}

function hideAuthForms() {
    document.getElementById('auth-forms').style.display = 'none';
    document.getElementById('message-input-container').style.display = 'block';
}

function register() {
    const username = document.getElementById('register-username').value.trim();
    const password = document.getElementById('register-password').value.trim();

    if (!username || !password) {
        addSystemMessage("Please enter both username and password");
        return;
    }

    if (window.external) {
        try {
            window.external.invoke(JSON.stringify({
                type: 'Register',
                username: username,
                password: password
            }));
        } catch (e) {
            console.error('Error registering:', e);
            addSystemMessage("Registration failed: " + e.message);
        }
    }
}

function login() {
    const username = document.getElementById('login-username').value.trim();
    const password = document.getElementById('login-password').value.trim();

    if (!username || !password) {
        addSystemMessage("Please enter both username and password");
        return;
    }

    if (window.external) {
        try {
            window.external.invoke(JSON.stringify({
                type: 'Login',
                username: username,
                password: password
            }));
        } catch (e) {
            console.error('Error logging in:', e);
            addSystemMessage("Login failed: " + e.message);
        }
    }
}

var currentBanNotification = null;
var banTimerInterval = null;

function addMessage(user, avatar, message, timestamp, isSystem) {
    var chat = document.getElementById('chat-messages');
    var messageDiv = document.createElement('div');
    messageDiv.className = 'message';
    
    var formattedTime = '';
    try {
        var date = new Date(timestamp);
        formattedTime = date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    } catch (e) {
        console.error('Error formatting timestamp:', e);
        formattedTime = 'now';
    }

    messageDiv.innerHTML = 
        '<div class="avatar">' + avatar + '</div>' +
        '<div class="message-content">' +
            '<div class="message-header">' +
                '<span class="username">' + user + '</span>' +
                '<span class="timestamp">' + formattedTime + '</span>' +
            '</div>' +
            '<div class="message-text' + (isSystem ? ' system-message' : '') + '">' + message + '</div>' +
        '</div>';
    
    chat.appendChild(messageDiv);
    chat.scrollTop = chat.scrollHeight;
}

function addSystemMessage(message) {
    addMessage("System", "⚡", message, new Date().toISOString(), true);
}

function disableInput(seconds) {
    var input = document.getElementById('message-input');
    if (input) {
        input.disabled = true;
        input.placeholder = 'Banned (' + seconds + 's remaining)';
        input.style.color = '#72767d';
        input.style.cursor = 'not-allowed';
    }
}

function enableInput() {
    var input = document.getElementById('message-input');
    if (input) {
        input.disabled = false;
        input.placeholder = 'Message #general';
        input.style.color = '#dcddde';
        input.style.cursor = 'text';
    }
}

function showBan(reason, seconds) {
    // Remove previous ban notification if exists
    if (currentBanNotification) {
        currentBanNotification.remove();
        currentBanNotification = null;
    }
    
    // Create new notification with static duration
    var chat = document.getElementById('chat-messages');
    currentBanNotification = document.createElement('div');
    currentBanNotification.className = 'notification ban-notification';
    currentBanNotification.textContent = '🚨 YOU WERE BANNED: ' + reason + ' (' + seconds + 's)';
    chat.appendChild(currentBanNotification);
    chat.scrollTop = chat.scrollHeight;
    
    // Clear any existing interval
    if (banTimerInterval) {
        clearInterval(banTimerInterval);
    }
    
    // Store the current seconds
    var remainingSeconds = parseInt(seconds);
    var input = document.getElementById('message-input');
    
    // Update the placeholder every second
    banTimerInterval = setInterval(function() {
        remainingSeconds--;
        
        // Only update the input placeholder
        if (input) {
            input.placeholder = 'Banned (' + remainingSeconds + 's remaining)';
        }
        
        // Check if ban has expired
        if (remainingSeconds <= 0) {
            clearInterval(banTimerInterval);
            if (currentBanNotification) {
                currentBanNotification.remove();
                currentBanNotification = null;
            }
            enableInput();
        }
    }, 1000);
}

document.getElementById('message-input').addEventListener('keydown', function(e) {
    if (e.key === 'Enter') {
        var message = this.value.trim();
        if (message && !this.disabled) {
            if (window.external) {
                try {
                    window.external.invoke(JSON.stringify({
                        type: 'Message',
                        content: message
                    }));
                } catch (e) {
                    console.error('Error sending message:', e);
                    addSystemMessage("Failed to send message");
                }
            }
            this.value = '';
        }
    }
});

function initChat() {
    if (window.external) {
        window.external.invoke(JSON.stringify({
            type: 'CheckAuth'
        }));
        window.external.invoke(JSON.stringify({
            type: 'RequestMessages'
        }));
    }
    addSystemMessage("Welcome to Gooncord! Every second there's a 5% chance you'll get banned for a funny reason.");

    setInterval(function() {
        if (window.external) {
            try {
                window.external.invoke(JSON.stringify({
                    type: 'BanStatus'
                }));
            } catch (e) {
                console.error('Error checking ban status:', e);
            }
        }
    }, 1000);
}

window.addEventListener('DOMContentLoaded', initChat);